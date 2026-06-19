//! Durable workflow run module.
//!
//! Postgres is the source of truth (`workflow_executions` + `workflow_node_runs`);
//! pgmq is a lease/wake mechanism only. A run can be resumed after a worker crash:
//! on redelivery the lease maps back to the in-flight execution, already-succeeded
//! nodes are skipped, and their outputs are reloaded from the DB. Per-node retry
//! with backoff and `continueOnError` are opt-in via node params.
//!
//! Guarantee is at-least-once: a node whose write committed but whose node_run was
//! not yet persisted before a crash re-runs on resume. Node tools must be idempotent.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value as JsonValue, json};
use sqlx::PgPool;
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::tools::ToolRegistry;
use rootcx_types::{
    WorkflowExecutionStatus, WorkflowGraph, WorkflowNode, WorkflowNodeKind, WorkflowNodeRunStatus,
};

use super::events::{WorkflowEvent, WorkflowEvents};
use super::executor;

const LEASE_VT_SECS: i32 = 120;

/// Keeps the pgmq lease alive while a node runs longer than the visibility timeout.
pub struct Heartbeat {
    pool: PgPool,
    msg_id: i64,
}

impl Heartbeat {
    pub fn lease(pool: PgPool, msg_id: i64) -> Self { Self { pool, msg_id } }

    fn spawn(&self) -> JoinHandle<()> {
        let (pool, msg_id) = (self.pool.clone(), self.msg_id);
        tokio::spawn(async move {
            let period = Duration::from_secs(LEASE_VT_SECS as u64 / 2);
            // Keep trying through transient errors: a single failed `set_vt` must not
            // surrender the lease while the run is still alive (it would let the
            // message redeliver and a second runner start concurrently).
            loop {
                tokio::time::sleep(period).await;
                if let Err(e) = crate::jobs::extend_lease(&pool, msg_id, LEASE_VT_SECS).await {
                    tracing::warn!(msg_id, "workflow lease heartbeat failed (will retry): {e}");
                }
            }
        })
    }
}

struct AbortOnDrop(JoinHandle<()>);
impl Drop for AbortOnDrop {
    fn drop(&mut self) { self.0.abort(); }
}

struct RetryPolicy {
    max_attempts: u32,
    backoff_ms: u64,
    continue_on_error: bool,
}

/// Per-node policy from `params.retry = {maxAttempts, backoffMs}` and
/// `params.continueOnError`. Default is a single attempt (no implicit retry — a
/// retried mutation could double-write); nodes opt in explicitly.
fn retry_policy(node: &WorkflowNode) -> RetryPolicy {
    let r = node.params.get("retry");
    RetryPolicy {
        max_attempts: r.and_then(|v| v.get("maxAttempts")).and_then(|v| v.as_u64()).unwrap_or(1).max(1) as u32,
        backoff_ms: r.and_then(|v| v.get("backoffMs")).and_then(|v| v.as_u64()).unwrap_or(500),
        continue_on_error: node.params.get("continueOnError").and_then(|v| v.as_bool()).unwrap_or(false),
    }
}

/// Snapshot the (enabled) workflow graph into a new execution row. The snapshot is
/// what the runner replays, so later edits / versioning never mutate a live run.
pub async fn create_execution(
    pool: &PgPool,
    workflow_id: Uuid,
    app_id: &str,
    user_id: Uuid,
    trigger_data: Option<JsonValue>,
    lease_msg_id: Option<i64>,
) -> Result<Uuid, String> {
    let graph_json: JsonValue = sqlx::query_scalar(
        "SELECT graph FROM rootcx_system.workflows WHERE id = $1 AND app_id = $2 AND enabled = true",
    ).bind(workflow_id).bind(app_id)
    .fetch_optional(pool).await.map_err(|e| e.to_string())?
    .ok_or_else(|| "workflow not found or not enabled".to_string())?;

    let mut graph: WorkflowGraph = serde_json::from_value(graph_json)
        .map_err(|e| format!("invalid graph: {e}"))?;

    // Trigger data becomes the trigger node's params (the run's seed input).
    if let Some(td) = &trigger_data {
        if let Some(t) = graph.nodes.iter_mut().find(|n| matches!(n.kind, WorkflowNodeKind::Trigger { .. })) {
            t.params = td.clone();
        }
    }
    let snapshot = serde_json::to_value(&graph).map_err(|e| e.to_string())?;

    sqlx::query_scalar(
        "INSERT INTO rootcx_system.workflow_executions
           (id, workflow_id, app_id, status, run_as_user_id, trigger_data, graph, lease_msg_id)
         VALUES (gen_random_uuid(), $1, $2, 'queued', $3, $4, $5, $6) RETURNING id",
    ).bind(workflow_id).bind(app_id).bind(user_id).bind(&trigger_data).bind(&snapshot).bind(lease_msg_id)
    .fetch_one(pool).await.map_err(|e| e.to_string())
}

/// The non-terminal execution currently bound to a pgmq lease, if any. On
/// redelivery this is the run to resume rather than starting a fresh one.
pub async fn inflight_for_lease(pool: &PgPool, msg_id: i64) -> Option<Uuid> {
    sqlx::query_scalar(
        "SELECT id FROM rootcx_system.workflow_executions
         WHERE lease_msg_id = $1 AND status IN ('queued', 'running')
         ORDER BY created_at DESC LIMIT 1",
    ).bind(msg_id).fetch_optional(pool).await.ok().flatten()
}

/// Drive an execution to a terminal state, resuming from persisted node_runs.
/// Returns the final status. `Err` means the run could not even start (load
/// failure) — the caller leaves the lease for redelivery. Per-node results live
/// in `workflow_node_runs` (durable) and on the event bus (live); no in-memory copy.
pub async fn run(
    registry: &Arc<ToolRegistry>,
    pool: &PgPool,
    exec_id: Uuid,
    user_id: Uuid,
    perms: &[String],
    hb: Heartbeat,
    events: &WorkflowEvents,
) -> Result<WorkflowExecutionStatus, String> {
    let (app_id, graph_json): (String, Option<JsonValue>) = sqlx::query_as(
        "SELECT app_id, graph FROM rootcx_system.workflow_executions WHERE id = $1",
    ).bind(exec_id).fetch_optional(pool).await.map_err(|e| e.to_string())?
    .ok_or_else(|| "execution not found".to_string())?;

    let graph: WorkflowGraph = serde_json::from_value(graph_json.ok_or("execution has no graph snapshot")?)
        .map_err(|e| format!("invalid graph snapshot: {e}"))?;

    if let Err(issues) = super::validate::validate(&graph) {
        return Err(format!("graph validation failed: {}", issues.join("; ")));
    }

    let _ticker = AbortOnDrop(hb.spawn());

    sqlx::query(
        "UPDATE rootcx_system.workflow_executions
         SET status = 'running', attempts = attempts + 1, started_at = COALESCE(started_at, now())
         WHERE id = $1",
    ).bind(exec_id).execute(pool).await.map_err(|e| e.to_string())?;

    let node_map: HashMap<&str, &WorkflowNode> =
        graph.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let order = executor::topo_sort(&graph);

    // Resume: rebuild caches from a prior (crashed) attempt so the loop skips
    // already-succeeded nodes and continues from their outputs.
    let prior: Vec<(String, String, Option<JsonValue>)> = sqlx::query_as(
        "SELECT node_id, status, output FROM rootcx_system.workflow_node_runs WHERE execution_id = $1",
    ).bind(exec_id).fetch_all(pool).await.map_err(|e| e.to_string())?;
    let Resume { mut active_ports, mut run_outputs, done } = seed_resume(prior);

    let mut all_succeeded = true;
    let mut first_error: Option<String> = None;
    let mut aborted = false;
    let mut halted = false;

    for node_id in &order {
        let Some(node) = node_map.get(node_id.as_str()) else { continue };
        if done.contains(node_id) { continue; }
        if !executor::should_execute(node_id, &graph, &active_ports) { continue; }

        let policy = retry_policy(node);
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let ex = executor::execute_node(registry, pool, &app_id, user_id, perms, node, &graph, &run_outputs, exec_id).await;

            // Retry a hard failure until attempts are exhausted.
            if ex.status == WorkflowNodeRunStatus::Failed && attempt < policy.max_attempts {
                persist_node_run(pool, exec_id, node_id, WorkflowNodeRunStatus::Failed, &ex.input_json, &ex.output_json, ex.error.as_deref(), attempt).await;
                tokio::time::sleep(Duration::from_millis(policy.backoff_ms * attempt as u64)).await;
                continue;
            }

            // continueOnError: record the failure but route an error item downstream
            // and let the run proceed (n8n's error-output behaviour).
            let (rec_status, output_json) = if ex.status == WorkflowNodeRunStatus::Failed && policy.continue_on_error {
                (WorkflowNodeRunStatus::Succeeded, json!([[{ "json": { "error": ex.error.clone().unwrap_or_default() } }]]))
            } else {
                (ex.status, ex.output_json)
            };

            persist_node_run(pool, exec_id, node_id, rec_status, &ex.input_json, &output_json, ex.error.as_deref(), attempt).await;
            events.publish(exec_id, WorkflowEvent::Node {
                node_id: node_id.clone(), status: rec_status.as_str().into(),
                output: output_json.clone(), error: ex.error.clone(),
            });

            if rec_status == WorkflowNodeRunStatus::Succeeded {
                active_ports.insert(node_id.clone(), ports_with_items(&output_json));
                run_outputs.insert(node_id.clone(), output_json);
            } else {
                all_succeeded = false;
                if first_error.is_none() { first_error = ex.error.clone().or_else(|| Some("node failed".into())); }
                if rec_status == WorkflowNodeRunStatus::Failed { aborted = true; }
            }
            // A Stop node halts the whole run, but as a success (not an abort).
            if ex.halt { halted = true; }
            break;
        }
        if aborted || halted { break; }
    }

    let final_status = if all_succeeded { WorkflowExecutionStatus::Succeeded } else { WorkflowExecutionStatus::Failed };

    sqlx::query(
        "UPDATE rootcx_system.workflow_executions
         SET status = $2, error = $3, finished_at = now(), lease_msg_id = NULL WHERE id = $1",
    ).bind(exec_id).bind(final_status.as_str()).bind(&first_error)
    .execute(pool).await.map_err(|e| e.to_string())?;

    events.publish(exec_id, WorkflowEvent::Done { status: final_status.as_str().into(), error: first_error });
    events.close(exec_id);

    Ok(final_status)
}

async fn persist_node_run(
    pool: &PgPool, exec_id: Uuid, node_id: &str, status: WorkflowNodeRunStatus,
    input: &JsonValue, output: &JsonValue, error: Option<&str>, attempt: u32,
) {
    let _ = sqlx::query(
        "INSERT INTO rootcx_system.workflow_node_runs
           (execution_id, node_id, status, input, output, error, attempts, started_at, finished_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, now(), now())
         ON CONFLICT (execution_id, node_id) DO UPDATE SET
           status = EXCLUDED.status, input = EXCLUDED.input, output = EXCLUDED.output,
           error = EXCLUDED.error, attempts = EXCLUDED.attempts, finished_at = now()",
    ).bind(exec_id).bind(node_id).bind(status.as_str())
    .bind(input).bind(output).bind(error).bind(attempt as i32)
    .execute(pool).await;
}

/// Caches rebuilt from a prior (crashed) attempt's node_runs. Only `succeeded`
/// nodes are replayed: their output reloads and they're marked `done` (skipped on
/// resume); a failed/pending/running node is absent from `done` and re-runs
/// (at-least-once). A succeeded node with no stored output is `done` but seeds no
/// active ports — so it can't wrongly re-fire downstream.
struct Resume {
    active_ports: HashMap<String, Vec<u8>>,
    run_outputs: HashMap<String, JsonValue>,
    done: HashSet<String>,
}

fn seed_resume(prior: Vec<(String, String, Option<JsonValue>)>) -> Resume {
    let mut r = Resume { active_ports: HashMap::new(), run_outputs: HashMap::new(), done: HashSet::new() };
    for (node_id, status, output) in prior {
        if status != WorkflowNodeRunStatus::Succeeded.as_str() { continue; }
        if let Some(out) = output {
            r.active_ports.insert(node_id.clone(), ports_with_items(&out));
            r.run_outputs.insert(node_id.clone(), out);
        }
        r.done.insert(node_id);
    }
    r
}

fn ports_with_items(output: &JsonValue) -> Vec<u8> {
    super::items::active_ports(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use rootcx_types::{TriggerKind, WorkflowNode};

    fn node(params: JsonValue) -> WorkflowNode {
        WorkflowNode {
            id: "n".into(), kind: WorkflowNodeKind::Trigger { trigger: TriggerKind::Manual },
            label: None, params, position: [0.0, 0.0],
        }
    }

    #[test]
    fn retry_policy_defaults_and_overrides() {
        let cases: Vec<(&str, JsonValue, u32, u64, bool)> = vec![
            ("default = single attempt, no continue", json!({}), 1, 500, false),
            ("explicit retry", json!({"retry": {"maxAttempts": 3, "backoffMs": 200}}), 3, 200, false),
            ("maxAttempts floored to 1", json!({"retry": {"maxAttempts": 0}}), 1, 500, false),
            ("continueOnError", json!({"continueOnError": true}), 1, 500, true),
        ];
        for (label, params, max, backoff, coe) in cases {
            let p = retry_policy(&node(params));
            assert_eq!(p.max_attempts, max, "[{label}] max");
            assert_eq!(p.backoff_ms, backoff, "[{label}] backoff");
            assert_eq!(p.continue_on_error, coe, "[{label}] coe");
        }
    }

    // The crash-resume contract: which prior node_runs are replayed vs re-run.
    // This is the durability guarantee that integration tests can't reach cheaply
    // (resume only fires on a 120s lease redelivery).
    #[test]
    fn seed_resume_replays_only_succeeded_with_output() {
        let prior = vec![
            ("a".into(), "succeeded".into(), Some(json!([[{"json": {"x": 1}}], []]))), // port 0 only
            ("b".into(), "failed".into(), Some(json!([[{"json": {}}]]))),              // must re-run
            ("c".into(), "succeeded".into(), None),                                    // done, but no output
            ("d".into(), "running".into(), None),                                      // crashed mid-flight → re-run
        ];
        let r = seed_resume(prior);

        // Succeeded-with-output: skipped on resume, output + ports reloaded.
        assert!(r.done.contains("a"));
        assert_eq!(r.active_ports.get("a"), Some(&vec![0u8]));
        assert_eq!(r.run_outputs.get("a"), Some(&json!([[{"json": {"x": 1}}], []])));

        // Succeeded-without-output: done (skipped) but seeds no ports/output.
        assert!(r.done.contains("c"));
        assert!(!r.active_ports.contains_key("c"));
        assert!(!r.run_outputs.contains_key("c"));

        // Not succeeded: absent from `done` so the runner re-runs it (at-least-once).
        assert!(!r.done.contains("b"), "failed node must re-run");
        assert!(!r.done.contains("d"), "mid-flight node must re-run");
        assert!(!r.run_outputs.contains_key("b") && !r.run_outputs.contains_key("d"));
    }
}
