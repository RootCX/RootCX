use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sqlx::PgPool;
use tokio::sync::{Notify, Semaphore};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::extensions::agents::persistence;
use crate::extensions::workflows::events::WorkflowEvents;
use crate::ipc::{AgentInvokePayload, LlmModelRef};
use crate::tools::ToolRegistry;
use crate::{jobs, worker_manager::WorkerManager};

const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Messages handled at once. Dispatch is concurrent because a single slow
/// dispatch used to block the whole queue: `get_or_spawn` starts a Bun process,
/// and every other app's crons and jobs waited behind that cold start. Bounded
/// because each permit may cost one process.
const MAX_CONCURRENT_DISPATCH: usize = 8;

/// The loop is considered stalled if it has not completed an iteration in this
/// long. The poll interval is 500ms and dispatch no longer blocks the loop, so
/// this is orders of magnitude past normal — it only fires on a wedged await.
const STALL_AFTER: Duration = Duration::from_secs(120);

/// How often the watchdog checks the tick clock.
const WATCHDOG_INTERVAL: Duration = Duration::from_secs(15);

/// Seconds since boot. An `Instant` is not representable as an atomic, and the
/// watchdog only needs a coarse monotonic tick. Tokio's clock rather than the
/// std one, so the watchdog is exercisable under `start_paused`.
fn now_secs(started: tokio::time::Instant) -> u64 {
    started.elapsed().as_secs()
}

pub struct SchedulerHandle {
    pub wake: Arc<Notify>,
    pub cancel: CancellationToken,
    /// Seconds-since-boot of the last completed loop iteration, and the boot
    /// instant it is measured against. Read by `/health?full` so a scheduler
    /// that has stopped moving is visible from outside the process — the
    /// failure this whole supervision layer exists for is silent, not loud.
    tick: Arc<AtomicU64>,
    started: tokio::time::Instant,
    restarts: Arc<AtomicU64>,
    permits: Arc<Semaphore>,
}

impl SchedulerHandle {
    /// `None` while healthy; `Some(seconds_since_last_tick)` once past
    /// `STALL_AFTER`.
    pub fn stalled_for(&self) -> Option<u64> {
        let gap = now_secs(self.started).saturating_sub(self.tick.load(Ordering::Relaxed));
        (gap > STALL_AFTER.as_secs()).then_some(gap)
    }

    pub fn restarts(&self) -> u64 {
        self.restarts.load(Ordering::Relaxed)
    }

    /// Dispatch slots free of [`MAX_CONCURRENT_DISPATCH`]. Zero for any length
    /// of time means the queue is backing up behind slow handlers, which is a
    /// different failure from a dead loop and must not be reported as one.
    pub fn free_slots(&self) -> usize {
        self.permits.available_permits()
    }
}

async fn dispatch_agent_job(
    pool: &PgPool,
    wm: &Arc<WorkerManager>,
    msg_id: i64,
    target_app: &str,
    message: String,
    invoker_user_id: Option<uuid::Uuid>,
    label: &'static str,
) {
    // Single owned-automation gate (B1): owner present + enabled + valid
    // delegation + still holds app:{id}:invoke. Fail-closed.
    if let Err(denied) = crate::governance::triggers::fire_gate::assert_can_fire(
        pool, invoker_user_id, target_app,
    ).await {
        warn!(msg_id, app_id = %target_app, "{label} agent denied: {denied}");
        let _ = jobs::fail(pool, msg_id).await;
        return;
    }

    let llm = crate::routes::llm_models::fetch_default_llm(pool).await
        .ok().flatten()
        .map(|(provider, model)| LlmModelRef { provider, model });

    let session_id = uuid::Uuid::new_v4().to_string();
    let user_message = message.clone();

    let invoke_payload = AgentInvokePayload {
        invoke_id: uuid::Uuid::new_v4().to_string(),
        session_id: session_id.clone(),
        message,
        history: vec![],
        is_sub_invoke: false,
        llm,
        invoker_user_id,
        attachments: None,
        task_scope: Some(vec![format!("app:{target_app}:*")]),
    };

    let system_user = uuid::Uuid::nil();
    let _ = persistence::ensure_session(pool, &session_id, target_app, system_user).await;
    let _ = persistence::persist_message(pool, &session_id, "user", &user_message, None, false).await;

    match wm.agent_invoke(target_app, invoke_payload, None).await {
        Ok(mut rx) => {
            let pool_c = pool.clone();
            let target_app_c = target_app.to_string();
            tokio::spawn(async move {
                while let Some(event) = rx.recv().await {
                    match event {
                        crate::worker::AgentEvent::ToolCallStarted { call_id, tool_name, input } => {
                            let _ = persistence::persist_tool_call_start(&pool_c, &session_id, &call_id, &tool_name, &input).await;
                        }
                        crate::worker::AgentEvent::ToolCallCompleted { call_id, output, error, duration_ms, .. } => {
                            let _ = persistence::persist_tool_call_end(&pool_c, &call_id, output.as_ref(), error.as_deref(), duration_ms).await;
                        }
                        crate::worker::AgentEvent::Done { response, tokens } => {
                            let _ = persistence::finalize_session(&pool_c, &session_id, &user_message, &response, tokens).await;
                            let _ = jobs::complete(&pool_c, msg_id).await;
                            info!(msg_id, app_id = %target_app_c, %session_id, "{label} agent completed");
                            return;
                        }
                        crate::worker::AgentEvent::Error { error } => {
                            error!(msg_id, app_id = %target_app_c, "{label} agent error: {error}");
                            let _ = jobs::fail(&pool_c, msg_id).await;
                            return;
                        }
                        _ => {}
                    }
                }
                let _ = jobs::fail(&pool_c, msg_id).await;
            });
        }
        Err(e) => {
            warn!(msg_id, "{label} agent dispatch failed: {e}");
            let _ = jobs::fail(pool, msg_id).await;
        }
    }
}

/// A workflow message is dead-lettered after this many deliveries (lease expiries).
const MAX_DELIVERIES: i32 = 5;

/// Message ids with a live run task. The runtime runs a single scheduler, so an
/// in-memory set is enough to enforce single-flight per lease and stop a
/// redelivered message from starting a second concurrent run of the same execution.
type InFlight = Arc<Mutex<HashSet<i64>>>;

/// Clears the in-flight mark on drop — including panic, so a crashed run frees its
/// lease for a clean redelivery/resume rather than wedging it forever.
struct InFlightGuard { set: InFlight, msg_id: i64 }
impl Drop for InFlightGuard {
    fn drop(&mut self) { self.set.lock().unwrap().remove(&self.msg_id); }
}

/// Resolve the responsible human's permissions, or drop the message (terminal —
/// perms won't change on redelivery).
async fn perms_or_fail(pool: &PgPool, msg_id: i64, uid: uuid::Uuid) -> Option<Vec<String>> {
    match crate::governance::authority::resolve_permissions(pool, uid).await {
        Ok((_, perms)) => Some(perms),
        Err(e) => {
            warn!(msg_id, "workflow perms: {e:?}");
            let _ = jobs::fail(pool, msg_id).await;
            None
        }
    }
}

/// Drive a non-terminal execution to `failed` and release its stream channel. The
/// status guard makes it a no-op once the run already finished (avoids clobbering a
/// concurrent terminal write).
async fn fail_execution(pool: &PgPool, events: &WorkflowEvents, exec_id: uuid::Uuid, reason: &str) {
    let _ = sqlx::query(
        "UPDATE rootcx_system.workflow_executions
         SET status = 'failed', error = $2, finished_at = now(), lease_msg_id = NULL
         WHERE id = $1 AND status NOT IN ('succeeded', 'failed', 'canceled')",
    ).bind(exec_id).bind(reason).execute(pool).await;
    events.close(exec_id);
}

/// Poison guard: fail the bound execution (if any) and move the message to the DLQ.
async fn dead_letter_workflow(pool: &PgPool, events: &WorkflowEvents, msg_id: i64, exec_id: Option<uuid::Uuid>, raw: serde_json::Value) {
    if let Some(id) = exec_id { fail_execution(pool, events, id, "exceeded max deliveries").await; }
    let _ = jobs::dead_letter(pool, msg_id, &raw, "exceeded max deliveries").await;
}

/// Drive a durable run under a lease heartbeat: archive the message on a terminal
/// outcome, or leave the lease to expire and redeliver on a transient error.
fn spawn_workflow_run(
    pool: PgPool, registry: Arc<ToolRegistry>, events: WorkflowEvents, in_flight: InFlight,
    msg_id: i64, exec_id: uuid::Uuid, uid: uuid::Uuid, perms: Vec<String>, label: &'static str,
) {
    use crate::extensions::workflows::runner;
    // Single-flight: if a run task for this lease is still alive (e.g. the lease was
    // lost mid-run and the message redelivered), refuse to start a second one —
    // concurrent runs would re-invoke nodes and double-fire side effects.
    if !in_flight.lock().unwrap().insert(msg_id) {
        debug!(msg_id, %exec_id, "{label} workflow already in flight; skipping duplicate delivery");
        return;
    }
    tokio::spawn(async move {
        let _guard = InFlightGuard { set: in_flight, msg_id };
        match runner::run(&registry, &pool, exec_id, uid, &perms, msg_id, &events).await {
            Ok(status) => {
                let _ = jobs::complete(&pool, msg_id).await;
                info!(msg_id, %exec_id, ?status, "{label} workflow finished");
            }
            Err(e) => warn!(msg_id, %exec_id, "{label} workflow run error (will retry): {e}"),
        }
    });
}

/// Triggered run (cron / record-change): resolve the workflow, gate via `fire_gate`
/// (run-as owner), then resume or snapshot an execution bound to this lease.
async fn dispatch_workflow_job(
    pool: &PgPool,
    tool_registry: &Arc<ToolRegistry>,
    msg_id: i64,
    read_ct: i32,
    workflow_id: &str,
    user_id: Option<uuid::Uuid>,
    trigger_data: serde_json::Value,
    events: &WorkflowEvents,
    in_flight: &InFlight,
    label: &'static str,
) {
    use crate::extensions::workflows::runner;

    let Ok(wf_uuid) = workflow_id.parse::<uuid::Uuid>() else {
        warn!(msg_id, workflow_id, "{label} workflow: invalid workflow_id");
        let _ = jobs::fail(pool, msg_id).await;
        return;
    };
    let app_id: String = match sqlx::query_scalar::<_, String>(
        "SELECT app_id FROM rootcx_system.workflows WHERE id = $1",
    ).bind(wf_uuid).fetch_optional(pool).await {
        Ok(Some(id)) => id,
        _ => {
            warn!(msg_id, workflow_id, "{label} workflow not found");
            let _ = jobs::fail(pool, msg_id).await;
            return;
        }
    };

    if read_ct > MAX_DELIVERIES {
        warn!(msg_id, read_ct, %app_id, "{label} workflow exceeded max deliveries → dead-letter");
        let raw = serde_json::json!({"app_id": app_id, "workflow_id": workflow_id, "user_id": user_id});
        dead_letter_workflow(pool, events, msg_id, runner::inflight_for_lease(pool, msg_id).await, raw).await;
        return;
    }

    let uid = match crate::governance::triggers::fire_gate::assert_can_fire_workflow(pool, user_id, &app_id).await {
        Ok(uid) => uid,
        Err(denied) => {
            warn!(msg_id, %app_id, "{label} workflow denied: {denied}");
            let _ = jobs::fail(pool, msg_id).await;
            return;
        }
    };
    let Some(perms) = perms_or_fail(pool, msg_id, uid).await else { return };

    // Resume the lease's in-flight execution (crash recovery) or snapshot a new one.
    let exec_id = match runner::inflight_for_lease(pool, msg_id).await {
        Some(id) => id,
        None => match runner::create_execution(pool, wf_uuid, &app_id, uid, Some(trigger_data), Some(msg_id)).await {
            Ok(id) => id,
            Err(e) => {
                warn!(msg_id, %app_id, "{label} workflow create failed: {e}");
                let _ = jobs::fail(pool, msg_id).await;
                return;
            }
        },
    };

    spawn_workflow_run(pool.clone(), Arc::clone(tool_registry), events.clone(), Arc::clone(in_flight), msg_id, exec_id, uid, perms, label);
}

/// Manual run: the execution row already exists (created by the HTTP handler, which
/// returned its id to the editor for streaming). Run-as caller — no `fire_gate`,
/// since the responsible human is the invoker. Resume by exec_id on redelivery.
async fn dispatch_manual_workflow(
    pool: &PgPool,
    tool_registry: &Arc<ToolRegistry>,
    events: &WorkflowEvents,
    in_flight: &InFlight,
    msg_id: i64,
    read_ct: i32,
    exec_id: uuid::Uuid,
    user_id: Option<uuid::Uuid>,
) {
    if read_ct > MAX_DELIVERIES {
        warn!(msg_id, read_ct, %exec_id, "manual workflow exceeded max deliveries → dead-letter");
        let raw = serde_json::json!({"manual": true, "execution_id": exec_id});
        dead_letter_workflow(pool, events, msg_id, Some(exec_id), raw).await;
        return;
    }

    // Denial is terminal: fail the (already-created) execution so it can't sit in
    // 'queued' forever, then drop the message.
    let Some(uid) = user_id else {
        warn!(msg_id, %exec_id, "manual workflow denied: no owner");
        fail_execution(pool, events, exec_id, "no responsible user").await;
        let _ = jobs::fail(pool, msg_id).await;
        return;
    };
    let Some(perms) = perms_or_fail(pool, msg_id, uid).await else {
        fail_execution(pool, events, exec_id, "permission resolution failed").await;
        return;
    };

    spawn_workflow_run(pool.clone(), Arc::clone(tool_registry), events.clone(), Arc::clone(in_flight), msg_id, exec_id, uid, perms, "manual");
}

/// Everything one message handler needs. Cloning is cheap (pool handle plus
/// Arcs), which is what lets a message be dispatched on its own task instead of
/// in the poll loop.
#[derive(Clone)]
struct Dispatcher {
    pool: PgPool,
    wm: Arc<WorkerManager>,
    tool_registry: Arc<ToolRegistry>,
    events: WorkflowEvents,
    in_flight: InFlight,
}

impl Dispatcher {
    /// Route one leased message. Every path is terminal for the message: it is
    /// archived, deleted, dead-lettered, or deliberately left to redeliver.
    async fn handle(&self, msg_id: i64, read_ct: i32, job_msg: jobs::JobMessage) {
        let Dispatcher { pool, wm, tool_registry, events, in_flight } = self;
        let is_hook = job_msg.payload.get("_hook").and_then(|v| v.as_bool()) == Some(true);
        let is_agent = job_msg.payload.get("action_type").and_then(|v| v.as_str()) == Some("agent");
        let is_workflow = job_msg.payload.get("action_type").and_then(|v| v.as_str()) == Some("workflow");

        // Manual run: execution already created by the HTTP handler.
        if is_workflow && job_msg.payload.get("manual").and_then(|v| v.as_bool()) == Some(true) {
            match job_msg.payload.get("execution_id").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()) {
                Some(exec_id) => dispatch_manual_workflow(pool, tool_registry, events, in_flight, msg_id, read_ct, exec_id, job_msg.user_id).await,
                None => {
                    warn!(msg_id, "manual workflow: missing execution_id");
                    let _ = jobs::fail(pool, msg_id).await;
                }
            }
            return;
        }

        if is_hook && is_agent {
            let target_app = job_msg.payload.get("action_config")
                .and_then(|c| c.get("app_id"))
                .and_then(|v| v.as_str())
                .unwrap_or(&job_msg.app_id)
                .to_string();

            let entity = job_msg.payload.get("entity").and_then(|v| v.as_str()).unwrap_or("unknown");
            let operation = job_msg.payload.get("operation").and_then(|v| v.as_str()).unwrap_or("unknown");
            let record = job_msg.payload.get("record").cloned().unwrap_or_default();
            let message = format!("Entity event: {operation} on {entity}\n\nRecord:\n{record}");

            dispatch_agent_job(pool, wm, msg_id, &target_app, message, job_msg.user_id, "hook").await;
            return;
        }

        if is_hook && is_workflow {
            let wf_id = job_msg.payload.get("action_config")
                .and_then(|c| c.get("workflow_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let trigger_data = serde_json::json!({
                "entity": job_msg.payload.get("entity"),
                "operation": job_msg.payload.get("operation"),
                "record": job_msg.payload.get("record"),
                "old_record": job_msg.payload.get("old_record"),
            });
            dispatch_workflow_job(pool, tool_registry, msg_id, read_ct, wf_id, job_msg.user_id, trigger_data, events, in_flight, "hook").await;
            return;
        }

        // Cron-triggered invocations.
        // Legacy payload cron_id remains routing-compatible, but only Core-owned
        // envelope provenance may grant job scope.
        let is_cron = job_msg.cron_id.is_some() || job_msg.payload.get("cron_id").is_some();
        let cron_workflow_id = job_msg.payload.get("workflow_id").and_then(|v| v.as_str());

        if let (true, Some(wf_id)) = (is_cron, cron_workflow_id) {
            dispatch_workflow_job(pool, tool_registry, msg_id, read_ct, wf_id, job_msg.user_id, serde_json::json!({"trigger": "schedule"}), events, in_flight, "cron").await;
            return;
        }

        let is_cron_agent = if is_cron {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM rootcx_system.agents WHERE app_id = $1)"
            ).bind(&job_msg.app_id).fetch_one(pool).await.unwrap_or(false)
        } else { false };

        if is_cron_agent {
            let message = job_msg.payload.get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("Scheduled invocation")
                .to_string();

            dispatch_agent_job(pool, wm, msg_id, &job_msg.app_id, message, job_msg.user_id, "cron").await;
            return;
        }

        // Regular job dispatch. Deny-by-default: a job with no owner
        // (created_by NULL) has no responsible human, so RLS would see no
        // identity. Refuse rather than fall back to admin.
        if jobs::delivery_exhausted(read_ct) {
            warn!(msg_id, read_ct, app_id = %job_msg.app_id,
                "app job exceeded max deliveries; moving to dead-letter queue");
            let raw = serde_json::to_value(&job_msg).unwrap_or_default();
            let reason = format!("exceeded {} deliveries", jobs::MAX_DELIVERIES);
            let _ = jobs::dead_letter(pool, msg_id, &raw, &reason).await;
            return;
        }

        let Some(uid) = job_msg.user_id else {
            warn!(msg_id, app_id = %job_msg.app_id,
                "job denied: no owner (created_by is NULL)");
            let _ = jobs::fail(pool, msg_id).await;
            return;
        };
        // The name is resolved only for a manifest cron that declared
        // `isolatedScope`, because that name becomes a dedicated worker process
        // and an unforgeable invocation identity in SQL. Every other schedule
        // runs on the app's shared worker with the invocation settings empty.
        // A cron created through the API is never in the manifest and so never
        // isolated: authority to pose an identity comes from the deployed
        // manifest, not from a runtime call.
        let cron_name = match job_msg.cron_id {
            Some(cron_id) => sqlx::query_scalar::<_, String>(
                "SELECT c.name
                   FROM rootcx_system.cron_schedules c
                   JOIN rootcx_system.apps a ON a.id = c.app_id
                  WHERE c.id = $1 AND c.app_id = $2
                    AND EXISTS (
                      SELECT 1 FROM jsonb_array_elements(
                        COALESCE(a.manifest->'crons', '[]'::jsonb)
                      ) d
                       WHERE d->>'name' = c.name
                         AND COALESCE((d->>'isolatedScope')::boolean, false)
                    )",
            )
            .bind(cron_id)
            .bind(&job_msg.app_id)
            .fetch_optional(pool)
            .await
            .unwrap_or(None),
            None => None,
        };
        let caller = crate::principal::resolve_caller(pool, uid).await;
        if let Err(e) = wm.dispatch_job(
            &job_msg.app_id,
            msg_id.to_string(),
            job_msg.payload,
            caller,
            cron_name.as_deref(),
        ).await {
            warn!(msg_id, "dispatch failed: {e}");
            let _ = jobs::fail(pool, msg_id).await;
        }
    }
}

/// Poll pgmq and hand each leased message to a bounded pool of dispatch tasks.
///
/// Returns only on cancellation. A panic escaping this future is caught by the
/// supervisor in [`spawn_scheduler`].
async fn scheduler_loop(
    dispatcher: Dispatcher,
    permits: Arc<Semaphore>,
    wake: Arc<Notify>,
    cancel: CancellationToken,
    tick: Arc<AtomicU64>,
    started: tokio::time::Instant,
) {
    loop {
        if cancel.is_cancelled() { break; }
        let Some(permit) = acquire_slot(&permits, &cancel, &tick, started).await else { break };

        match jobs::read_next(&dispatcher.pool).await {
            Ok(Some((msg_id, read_ct, job_msg))) => {
                let dispatcher = dispatcher.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    dispatcher.handle(msg_id, read_ct, job_msg).await;
                });
                // A non-empty queue keeps draining at full concurrency, without
                // paying the poll interval between messages.
                continue;
            }
            Ok(None) => {}
            Err(e) => error!("scheduler: {e}"),
        }

        // Nothing to run: release the slot rather than hold it across the idle
        // wait, where it would shrink the pool for no work.
        drop(permit);
        if wait_tick(&wake, &cancel).await { break; }
    }
    info!("job scheduler loop stopped");
}

/// Wait for a dispatch slot. `None` means the loop should stop.
///
/// Taken BEFORE reading a message, because a message read is a message leased:
/// leasing work with no capacity to start it burns deliveries against the
/// redelivery limit while nothing runs.
///
/// The tick keeps advancing while waiting. A saturated pool is not a dead loop,
/// and letting the watchdog abort one that is merely at capacity would interrupt
/// dispatches for no reason.
async fn acquire_slot(
    permits: &Arc<Semaphore>,
    cancel: &CancellationToken,
    tick: &AtomicU64,
    started: tokio::time::Instant,
) -> Option<tokio::sync::OwnedSemaphorePermit> {
    loop {
        tick.store(now_secs(started), Ordering::Relaxed);
        tokio::select! {
            permit = Arc::clone(permits).acquire_owned() => return permit.ok(),
            _ = cancel.cancelled() => return None,
            _ = tokio::time::sleep(WATCHDOG_INTERVAL) => {}
        }
    }
}

/// Idle between polls. `true` means cancelled.
async fn wait_tick(wake: &Notify, cancel: &CancellationToken) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(POLL_INTERVAL) => false,
        _ = wake.notified() => { debug!("scheduler woken"); false }
        _ = cancel.cancelled() => true,
    }
}

/// Start the scheduler under supervision.
///
/// The loop used to be a bare `tokio::spawn`. A panic inside it, or an await
/// that never returned, ended every job, cron and workflow for the tenant with
/// no error, no restart, and nothing in the health endpoint to show for it: the
/// pod stayed green while all automation was dead. The supervisor restarts the
/// loop on panic, aborts and restarts it when the tick clock stalls, and
/// publishes both facts for `/health?full`.
pub fn spawn_scheduler(pool: PgPool, wm: Arc<WorkerManager>, tool_registry: Arc<ToolRegistry>, events: WorkflowEvents) -> SchedulerHandle {
    let wake = Arc::new(Notify::new());
    let cancel = CancellationToken::new();
    let tick = Arc::new(AtomicU64::new(0));
    let restarts = Arc::new(AtomicU64::new(0));
    let started = tokio::time::Instant::now();
    let permits = Arc::new(Semaphore::new(MAX_CONCURRENT_DISPATCH));

    let dispatcher = Dispatcher {
        pool, wm, tool_registry, events,
        in_flight: Arc::new(Mutex::new(HashSet::new())),
    };

    let (w, c, t, r) = (Arc::clone(&wake), cancel.clone(), Arc::clone(&tick), Arc::clone(&restarts));
    let p = Arc::clone(&permits);
    tokio::spawn(supervise(c.clone(), t, r, started, move |tick| {
        scheduler_loop(dispatcher.clone(), Arc::clone(&p), Arc::clone(&w), c.clone(), tick, started)
    }));

    SchedulerHandle { wake, cancel, tick, started, restarts, permits }
}

/// Run `task` forever under supervision: restart it if it panics, abort and
/// restart it if it stops advancing `tick`, and stop when cancelled.
///
/// Generic over the task so the supervision itself is testable without a
/// database — the failure it guards against (a loop that dies or wedges in
/// silence) is precisely the one no integration test would ever provoke.
async fn supervise<F, Fut>(
    cancel: CancellationToken,
    tick: Arc<AtomicU64>,
    restarts: Arc<AtomicU64>,
    started: tokio::time::Instant,
    task: F,
) where
    F: Fn(Arc<AtomicU64>) -> Fut,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    info!("job scheduler started");
    while !cancel.is_cancelled() {
        tick.store(now_secs(started), Ordering::Relaxed);
        let mut run = tokio::spawn(task(Arc::clone(&tick)));

        let stalled = loop {
            tokio::select! {
                joined = &mut run => {
                    if let Err(e) = joined { error!("scheduler loop panicked, restarting: {e}"); }
                    break None;
                }
                _ = cancel.cancelled() => { run.abort(); break None; }
                _ = tokio::time::sleep(WATCHDOG_INTERVAL) => {
                    let gap = now_secs(started).saturating_sub(tick.load(Ordering::Relaxed));
                    if gap > STALL_AFTER.as_secs() { break Some(gap); }
                }
            }
        };

        if let Some(gap) = stalled {
            // Aborting mid-await is safe: an interrupted dispatch leaves its pgmq
            // message leased, and the lease is exactly the mechanism that
            // redelivers it.
            error!(stalled_secs = gap, "scheduler loop stalled, restarting");
            run.abort();
        }
        if cancel.is_cancelled() { break; }
        restarts.fetch_add(1, Ordering::Relaxed);
        // A restart storm helps nobody; give the cause a moment to clear.
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    info!("job scheduler stopped");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn handle(tick: u64, started: tokio::time::Instant) -> SchedulerHandle {
        SchedulerHandle {
            wake: Arc::new(Notify::new()),
            cancel: CancellationToken::new(),
            tick: Arc::new(AtomicU64::new(tick)),
            started,
            restarts: Arc::new(AtomicU64::new(0)),
            permits: Arc::new(Semaphore::new(MAX_CONCURRENT_DISPATCH)),
        }
    }

    // The whole point of the tick clock: a scheduler that has stopped moving is
    // reportable. If `stalled_for` ever returned None unconditionally, the
    // health endpoint would go back to claiming a dead scheduler is healthy.
    #[test]
    fn a_scheduler_that_stopped_ticking_is_reported_stalled() {
        let started = tokio::time::Instant::now() - (STALL_AFTER * 3);
        assert_eq!(
            handle(now_secs(started), started).stalled_for(),
            None,
            "a scheduler that just ticked is not stalled",
        );
        assert!(
            handle(0, started).stalled_for().is_some(),
            "a scheduler whose last tick predates the stall threshold must be reported",
        );
    }

    // A panicking loop used to end every job, cron and workflow for the tenant,
    // permanently and silently. Supervision must bring it back.
    #[tokio::test(start_paused = true)]
    async fn a_panicking_loop_is_restarted() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let cancel = CancellationToken::new();
        let restarts = Arc::new(AtomicU64::new(0));
        let started = tokio::time::Instant::now();

        let (a, c) = (Arc::clone(&attempts), cancel.clone());
        let sup = tokio::spawn(supervise(
            cancel.clone(), Arc::new(AtomicU64::new(0)), Arc::clone(&restarts), started,
            move |_| {
                let (a, c) = (Arc::clone(&a), c.clone());
                async move {
                    if a.fetch_add(1, Ordering::Relaxed) < 2 { panic!("boom"); }
                    c.cancel();
                    std::future::pending::<()>().await;
                }
            },
        ));

        tokio::time::timeout(Duration::from_secs(60), sup).await
            .expect("supervisor must settle").unwrap();
        assert_eq!(attempts.load(Ordering::Relaxed), 3, "each panic must be retried");
        assert_eq!(restarts.load(Ordering::Relaxed), 2, "restarts must be counted for /health");
    }

    // The failure a restart-on-panic does NOT catch: a loop that is alive but
    // wedged on an await. Without the tick watchdog it would hang forever.
    #[tokio::test(start_paused = true)]
    async fn a_wedged_loop_is_aborted_and_restarted() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let cancel = CancellationToken::new();
        let started = tokio::time::Instant::now();

        let (a, c) = (Arc::clone(&attempts), cancel.clone());
        let sup = tokio::spawn(supervise(
            cancel.clone(), Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)), started,
            move |tick| {
                let (a, c) = (Arc::clone(&a), c.clone());
                async move {
                    // Second attempt ticks and stops the test; the first never
                    // ticks and never returns.
                    if a.fetch_add(1, Ordering::Relaxed) > 0 {
                        tick.store(u64::MAX, Ordering::Relaxed);
                        c.cancel();
                    }
                    std::future::pending::<()>().await;
                }
            },
        ));

        tokio::time::timeout(STALL_AFTER * 4, sup).await
            .expect("a loop that stops ticking must be aborted, not awaited forever")
            .unwrap();
        assert_eq!(attempts.load(Ordering::Relaxed), 2);
    }
}
