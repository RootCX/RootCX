//! In-process pub/sub for live workflow execution progress.
//!
//! The runner (in the scheduler) publishes per-node results and a terminal event
//! keyed by execution id; the SSE route subscribes. Runner and HTTP server share
//! one process (the core), so a broadcast channel is enough — no external broker.
//! Late subscribers first replay persisted node_runs from the DB, then attach
//! here for the rest, so nothing is missed regardless of connect timing.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde_json::Value as JsonValue;
use tokio::sync::broadcast;
use uuid::Uuid;

#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum WorkflowEvent {
    Node { node_id: String, status: String, output: JsonValue, error: Option<String> },
    Done { status: String, error: Option<String> },
}

/// Workflow-level event: wraps a per-execution event with its exec_id so the
/// editor can watch all runs of a workflow in real time (webhook, cron, etc.).
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveEvent {
    pub execution_id: Uuid,
    #[serde(flatten)]
    pub event: WorkflowEvent,
}

/// One broadcast channel per in-flight execution. Cheap to clone (Arc inside).
#[derive(Clone, Default)]
pub struct WorkflowEvents {
    exec_chans: Arc<Mutex<HashMap<Uuid, broadcast::Sender<WorkflowEvent>>>>,
    wf_chans: Arc<Mutex<HashMap<Uuid, broadcast::Sender<LiveEvent>>>>,
}

impl WorkflowEvents {
    fn exec_sender(&self, exec_id: Uuid) -> broadcast::Sender<WorkflowEvent> {
        self.exec_chans.lock().unwrap().entry(exec_id)
            .or_insert_with(|| broadcast::channel(256).0).clone()
    }

    pub fn subscribe(&self, exec_id: Uuid) -> broadcast::Receiver<WorkflowEvent> {
        self.exec_sender(exec_id).subscribe()
    }

    /// Subscribe to all runs of a workflow (the editor uses this to light up nodes
    /// in real time regardless of who triggered the run).
    pub fn subscribe_workflow(&self, workflow_id: Uuid) -> broadcast::Receiver<LiveEvent> {
        self.wf_chans.lock().unwrap().entry(workflow_id)
            .or_insert_with(|| broadcast::channel(256).0)
            .subscribe()
    }

    /// Best-effort publish to both per-exec and per-workflow channels.
    pub fn publish(&self, exec_id: Uuid, workflow_id: Uuid, ev: WorkflowEvent) {
        let _ = self.exec_sender(exec_id).send(ev.clone());
        if let Some(tx) = self.wf_chans.lock().unwrap().get(&workflow_id) {
            let _ = tx.send(LiveEvent { execution_id: exec_id, event: ev });
        }
    }

    /// Drop the per-exec channel once terminal so the map doesn't grow.
    pub fn close(&self, exec_id: Uuid) {
        self.exec_chans.lock().unwrap().remove(&exec_id);
    }
}
