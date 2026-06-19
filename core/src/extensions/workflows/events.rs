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
#[serde(tag = "type", rename_all = "camelCase")]
pub enum WorkflowEvent {
    Node { node_id: String, status: String, output: JsonValue, error: Option<String> },
    Done { status: String, error: Option<String> },
}

/// One broadcast channel per in-flight execution. Cheap to clone (Arc inside).
#[derive(Clone, Default)]
pub struct WorkflowEvents {
    chans: Arc<Mutex<HashMap<Uuid, broadcast::Sender<WorkflowEvent>>>>,
}

impl WorkflowEvents {
    fn sender(&self, exec_id: Uuid) -> broadcast::Sender<WorkflowEvent> {
        self.chans.lock().unwrap().entry(exec_id)
            .or_insert_with(|| broadcast::channel(256).0).clone()
    }

    pub fn subscribe(&self, exec_id: Uuid) -> broadcast::Receiver<WorkflowEvent> {
        self.sender(exec_id).subscribe()
    }

    /// Best-effort: no subscribers (or a lagging one) is fine — the SSE route
    /// reconciles from the persisted node_runs, which remain the source of truth.
    pub fn publish(&self, exec_id: Uuid, ev: WorkflowEvent) {
        let _ = self.sender(exec_id).send(ev);
    }

    /// Drop the channel once an execution is terminal so the map doesn't grow.
    pub fn close(&self, exec_id: Uuid) {
        self.chans.lock().unwrap().remove(&exec_id);
    }
}
