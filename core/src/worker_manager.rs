use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use futures::future::join_all;
use serde_json::Value as JsonValue;
use sqlx::PgPool;
use tokio::sync::{RwLock, Semaphore, broadcast, mpsc};
use tracing::{error, info, warn};

use crate::RuntimeError;
use crate::extensions::agents::approvals::PendingApprovals;
use crate::extensions::logs::LogEntry;
use crate::ipc::{AgentBootConfig, AgentInvokePayload, LlmModelRef, RpcCaller};
use crate::secrets::SecretManager;
use crate::tools::{ActionCaller, AgentDispatcher, IntegrationCaller, ToolRegistry};
use crate::worker::{self, AgentEvent, FleetEvent, SupervisorHandle, WorkerConfig, WorkerStatus};
use crate::governance::enforcement::InvocationContext;

const BACKEND_PRELUDE: &str = include_str!("backend_prelude.js");

/// A worker process is keyed by (app_id, identity). One process serves exactly
/// ONE identity for its whole life, so a malicious app can never act as another
/// user (cross-user confused deputy is structurally impossible) and there is no
/// token to forge. See docs/security-context-token-confusion.md.
type WorkerKey = (String, String);

fn worker_key(
    app_id: &str,
    principal: &Principal,
    invocation: &InvocationContext,
) -> WorkerKey {
    (
        app_id.to_string(),
        format!("{}|scope={}", principal.key(), invocation.key()),
    )
}

fn worker_key_belongs_to_principal(key: &WorkerKey, user_id: uuid::Uuid) -> bool {
    if crate::extensions::agents::agent_user_id(&key.0) == user_id {
        return true;
    }
    let user_prefix = user_id.to_string();
    key.1 == user_prefix || key.1.starts_with(&format!("{user_prefix}|"))
}

/// Who a worker process acts as, for its whole life. Each distinct principal
/// gets its own process, so a worker can never act as another (the cross-user
/// confused deputy is structurally impossible). Three kinds never share a
/// process: the privileged lifecycle worker, un-authenticated traffic, and each
/// real authenticated identity.
enum Principal {
    /// The per-app lifecycle worker: runs onStart with BYPASSRLS self-schema.
    /// Spawned only by `start_app`, never by an incoming request.
    System,
    /// A request with no authenticated user (public/share-token RPC, owner-less
    /// webhook/job). Denied every row by RLS, and kept OFF the System worker so
    /// untrusted anonymous traffic never shares the privileged onStart process.
    Anonymous,
    /// A real identity: a direct user, or an agent's delegated authority.
    User(crate::governance::enforcement::ContextState),
}

impl Principal {
    /// Classify the identity resolved for an incoming request. A request never
    /// yields System; an empty identity (no user, not delegated) is Anonymous.
    fn from_request(state: crate::governance::enforcement::ContextState) -> Self {
        if state.user_id.is_none() && !state.is_delegated && state.effective_perms.is_empty() {
            Principal::Anonymous
        } else {
            Principal::User(state)
        }
    }

    /// Stable per-app worker key. Distinct principals never collide; the same
    /// User identity (perms in any order) always maps to the same worker.
    fn key(&self) -> String {
        match self {
            Principal::System => "·system".into(),
            Principal::Anonymous => "·anon".into(),
            Principal::User(s) => {
                let uid = s.user_id.map(|u| u.to_string()).unwrap_or_default();
                let mut perms = s.effective_perms.clone();
                perms.sort();
                format!("{uid}|{}|{}", s.is_delegated as u8, perms.join(","))
            }
        }
    }

    /// Only the lifecycle worker runs onStart / may BYPASSRLS the self-schema.
    fn run_onstart(&self) -> bool { matches!(self, Principal::System) }

    /// The RLS identity posed for this principal. System and Anonymous carry no
    /// user, so RLS denies every row; User poses its real identity.
    fn rls_state(&self) -> crate::governance::enforcement::ContextState {
        match self {
            Principal::User(s) => s.clone(),
            _ => crate::governance::enforcement::ContextState::default(),
        }
    }
}

/// How long a worker may sit unused before it is eligible for reaping. Generous
/// on purpose: respawning costs a process start, and the point is to bound an
/// unbounded set, not to churn.
const WORKER_IDLE_TTL: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// How often the reaper sweeps.
const REAP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

/// Memory budgeted per worker process. Measured at 36 MB marginal / 70 MB RSS
/// for a real customer app; the higher figure is the one to plan against, since
/// the OOM killer counts RSS.
const WORKER_MEMORY_BUDGET: u64 = 70 * 1024 * 1024;

/// Held back for the core process itself, its connection pool and page cache.
const CORE_MEMORY_RESERVE: u64 = 160 * 1024 * 1024;

/// Floor and ceiling on the derived cap. The floor keeps a very small pod usable
/// (one lifecycle worker plus one caller); the ceiling stops a large pod from
/// deriving a number so high the limit stops meaning anything.
const MIN_WORKERS: usize = 2;
const MAX_WORKERS: usize = 256;

/// Used when the cgroup limit is unreadable, which in practice means a
/// developer machine rather than a tenant pod.
const DEFAULT_WORKER_CAP: usize = 16;

/// How many workers admission may probe for idleness before giving up. Each
/// probe of a WEDGED worker costs the 2s `is_idle` timeout, and a caller waiting
/// on a full sweep of them is a worse outcome than a prompt refusal.
const MAX_EVICTION_PROBES: usize = 4;

/// How many worker processes this pod may host at once.
///
/// Derived from the cgroup memory limit rather than hardcoded, because the same
/// binary runs on a 512 MiB tenant and a 32 GiB one. `ROOTCX_MAX_WORKERS`
/// overrides it for operators; an unreadable limit (dev machines, non-cgroup
/// hosts) falls back to `default_cap`.
fn worker_cap(limit_bytes: Option<u64>, override_env: Option<&str>, default_cap: usize) -> usize {
    if let Some(n) = override_env.and_then(|v| v.trim().parse::<usize>().ok()) {
        return n.clamp(MIN_WORKERS, MAX_WORKERS);
    }
    let Some(limit) = limit_bytes else { return default_cap };
    let budgeted = limit.saturating_sub(CORE_MEMORY_RESERVE) / WORKER_MEMORY_BUDGET;
    (budgeted as usize).clamp(MIN_WORKERS, MAX_WORKERS)
}

/// The pod's memory ceiling, or `None` off cgroup v2.
fn cgroup_memory_limit() -> Option<u64> {
    std::fs::read_to_string("/sys/fs/cgroup/memory.max")
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
}

/// How often a draining worker is asked whether it has finished.
const DRAIN_POLL: std::time::Duration = std::time::Duration::from_millis(250);

/// Grace for a credential-rotation restart. Shorter than shutdown's: the process
/// is not going away, so a worker that overruns is simply replaced.
const RESTART_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// Wait until `handle` reports nothing in flight, or `deadline` passes.
///
/// A worker that never answers counts as busy (`is_idle` has its own timeout),
/// so a wedged one costs the full grace rather than being killed at once. That
/// is the right trade at shutdown: the alternative is killing work that was
/// about to commit.
async fn drain_until_idle(handle: &SupervisorHandle, deadline: tokio::time::Instant) {
    while tokio::time::Instant::now() < deadline {
        if handle.is_idle().await {
            return;
        }
        tokio::time::sleep(DRAIN_POLL).await;
    }
    warn!("worker still busy at the end of the shutdown grace; stopping it anyway");
}

/// A live worker and when it was last handed a unit of work.
///
/// The map is keyed by `(app, principal|scope)` and the principal embeds the
/// caller's permission set, so it grows with every distinct identity AND with
/// every role edit — a set that only ever grew, pruned solely by deploy or by a
/// later request for the same key noticing the handle had died. On a shared core
/// that is a host-wide OOM reached by ordinary use, and the OOM killer does not
/// stop at the tenant that grew.
#[derive(Clone)]
struct WorkerEntry {
    handle: SupervisorHandle,
    last_used: std::time::Instant,
    /// The pod slot this process occupies. Held by the entry so that EVERY way a
    /// worker leaves the map returns its slot, including ones added later: the
    /// reaper, `stop_app`, permission invalidation, and replacing a dead handle.
    /// Counting by hand instead is how a cap silently drifts shut.
    _slot: Arc<tokio::sync::OwnedSemaphorePermit>,
}

pub struct WorkerManager {
    workers: Arc<RwLock<HashMap<WorkerKey, WorkerEntry>>>,
    /// One permit per hostable worker process. The pool used to be unbounded,
    /// so ordinary use grew it until the pod was OOM-killed, which takes down
    /// every app of the tenant and every unit of work in flight. Refusing one
    /// caller is the better failure.
    slots: Arc<Semaphore>,
    pool: PgPool,
    dispatch: OnceLock<Arc<dyn AgentDispatcher>>,
    integration_call: OnceLock<Arc<dyn IntegrationCaller>>,
    action_call: OnceLock<Arc<dyn ActionCaller>>,
    fleet_tx: broadcast::Sender<FleetEvent>,
    apps_dir: PathBuf,
    prelude_path: PathBuf,
    runtime_url: String,
    bun_bin: PathBuf,
    tool_registry: Arc<ToolRegistry>,
    pending_approvals: PendingApprovals,
    secret_manager: Arc<SecretManager>,
    upload_nonces: Arc<std::sync::Mutex<crate::extensions::storage::nonce::NonceStore>>,
}

impl WorkerManager {
    pub fn new(
        apps_dir: PathBuf, runtime_url: String, bun_bin: PathBuf, pool: PgPool,
        tool_registry: Arc<ToolRegistry>, pending_approvals: PendingApprovals,
        secret_manager: Arc<SecretManager>,
        upload_nonces: Arc<std::sync::Mutex<crate::extensions::storage::nonce::NonceStore>>,
        max_workers: Option<usize>,
    ) -> Self {
        let prelude_path = apps_dir.join(".prelude.js");
        std::fs::write(&prelude_path, BACKEND_PRELUDE).expect("write backend prelude");
        let (fleet_tx, _) = broadcast::channel(512);
        let cap = max_workers.unwrap_or_else(|| {
            worker_cap(
                cgroup_memory_limit(),
                std::env::var("ROOTCX_MAX_WORKERS").ok().as_deref(),
                DEFAULT_WORKER_CAP,
            )
        });
        info!(cap, "worker pool sized");
        Self {
            workers: Arc::new(RwLock::new(HashMap::new())),
            slots: Arc::new(Semaphore::new(cap)),
            pool,
            dispatch: OnceLock::new(),
            action_call: OnceLock::new(),
            integration_call: OnceLock::new(),
            fleet_tx,
            apps_dir, prelude_path, runtime_url, bun_bin,
            tool_registry, pending_approvals, secret_manager, upload_nonces,
        }
    }

    /// Reap workers idle past `WORKER_IDLE_TTL`. Spawned once, alongside
    /// `init_self_ref`, and lives as long as the manager.
    ///
    /// Idleness is asked of the supervisor rather than inferred from `last_used`,
    /// because the manager only knows when work was DISPATCHED. A job running
    /// longer than the TTL, or an open transaction, would otherwise be killed
    /// mid-flight — turning a memory fix into data loss. A worker that does not
    /// answer counts as busy and is left alone.
    pub fn spawn_reaper(self: &Arc<Self>) {
        let wm = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(REAP_INTERVAL).await;
                let now = std::time::Instant::now();
                let stale: Vec<(WorkerKey, SupervisorHandle)> = wm.workers.read().await.iter()
                    .filter(|(_, e)| now.duration_since(e.last_used) > WORKER_IDLE_TTL)
                    .map(|(k, e)| (k.clone(), e.handle.clone()))
                    .collect();

                for (key, handle) in stale {
                    if !handle.is_idle().await {
                        continue;
                    }
                    // Re-check under the write lock: a request may have arrived
                    // between the idle probe and here, and it would have refreshed
                    // `last_used`. Dropping the entry then would strand a worker
                    // the map no longer knows about.
                    let mut w = wm.workers.write().await;
                    let still_stale = w.get(&key)
                        .is_some_and(|e| now.duration_since(e.last_used) > WORKER_IDLE_TTL);
                    if !still_stale {
                        continue;
                    }
                    w.remove(&key);
                    drop(w);
                    let _ = handle.stop().await;
                    info!(app_id = %key.0, "reaped idle worker");
                }
            }
        });
    }

    /// Must be called after wrapping in Arc to enable sub-agent dispatch and integration calling.
    pub fn init_self_ref(self: &Arc<Self>) {
        let _ = self.dispatch.set(Arc::new(SubAgentDispatch { wm: Arc::clone(self) }));
        let _ = self.integration_call.set(Arc::new(IntegrationCallImpl {
            wm: Arc::clone(self), secrets: Arc::clone(&self.secret_manager),
        }));
        let _ = self.action_call.set(Arc::new(AppActionCallImpl { wm: Arc::clone(self) }));
    }

    async fn build_agent_boot(&self, pool: &PgPool, app_id: &str) -> Option<(AgentBootConfig, Option<rootcx_types::SupervisionConfig>)> {
        let config_json: serde_json::Value = sqlx::query_scalar(
            "SELECT config FROM rootcx_system.agents WHERE app_id = $1",
        ).bind(app_id).fetch_optional(pool).await.ok()??;

        let agent_uid = crate::extensions::agents::agent_user_id(app_id);
        let (contract_res, perms_res) = tokio::join!(
            sqlx::query_scalar::<_, serde_json::Value>(
                "SELECT COALESCE(manifest->'dataContract', '[]'::jsonb) FROM rootcx_system.apps WHERE id = $1",
            ).bind(app_id).fetch_optional(pool),
            crate::governance::authority::resolve_permissions(pool, agent_uid),
        );

        let data_contract = contract_res.ok()?.unwrap_or_default();
        let (_, perms) = match perms_res {
            Ok(p) => p,
            Err(e) => { warn!(app_id, "agent boot: failed to resolve permissions: {e:?}"); return None; }
        };
        let tool_descriptors = self.tool_registry.descriptors_for_permissions(&perms, &data_contract);

        let max_turns = config_json.get("limits")
            .and_then(|l| l.get("maxTurns")).and_then(|v| v.as_u64()).unwrap_or(50) as u32;

        let supervision = config_json.get("supervision")
            .and_then(|v| serde_json::from_value::<rootcx_types::SupervisionConfig>(v.clone()).ok());

        Some((AgentBootConfig { tool_descriptors, max_turns }, supervision))
    }

    /// Spawn a fresh worker bound for life to `principal`. Only the System
    /// principal gets `run_onstart` (BYPASSRLS self-schema for onStart).
    async fn spawn_for(
        &self, pool: &PgPool, secrets: &SecretManager, app_id: &str,
        principal: &Principal, invocation: &InvocationContext,
    ) -> Result<SupervisorHandle, RuntimeError> {
        let app_dir = self.apps_dir.join(app_id);
        let entry_point = resolve_entry_point(&app_dir)?;
        let mut credentials = secrets.get_env_for_app(pool, app_id).await?;
        apply_openai_base_url(
            &mut credentials,
            std::env::var("ROOTCX_OPENAI_BASE_URL").ok(),
        );
        let (agent_boot_config, supervision) = match self.build_agent_boot(pool, app_id).await {
            Some((boot, sup)) => (Some(boot), sup),
            None => (None, None),
        };
        let mut identity = principal.rls_state();
        if identity.audit_actor_id.is_none() {
            if identity.is_delegated {
                identity.audit_actor_id = Some(crate::extensions::agents::agent_user_id(app_id));
                identity.audit_delegator_id = identity.user_id;
            } else {
                identity.audit_actor_id = identity.user_id;
            }
        }
        let config = WorkerConfig {
            app_id: app_id.to_string(),
            identity,
            invocation: invocation.clone(),
            run_onstart: principal.run_onstart(),
            entry_point,
            working_dir: app_dir,
            credentials,
            runtime_url: self.runtime_url.clone(),
            pool: pool.clone(),
            js_runtime: self.bun_bin.clone(),
            prelude_path: self.prelude_path.clone(),
            tool_registry: Arc::clone(&self.tool_registry),
            pending_approvals: self.pending_approvals.clone(),
            agent_dispatch: self.dispatch.get().cloned(),
            integration_caller: self.integration_call.get().cloned(),
            action_caller: self.action_call.get().cloned(),
            agent_boot_config,
            supervision,
            upload_nonces: Arc::clone(&self.upload_nonces),
        };
        // No protocol floor here. A scoped unit of work (an action, a job) is not a
        // new execution path a worker has to be certified for — it is how the
        // product has always dispatched. Refusing a worker that announces no
        // version took down every declared action and every cron on every worker
        // written before the version field existed. What v4 buys is the invocation
        // echo, and `worker::capability_context` decides that per message.
        let handle = worker::spawn_supervisor(config);
        handle.start().await?;
        Ok(handle)
    }

    /// Take a pod slot for a new worker, evicting to make room if need be.
    ///
    /// The reaper alone is not enough: it only runs every minute and only
    /// removes workers idle for fifteen, so a burst of new identities reaches
    /// the cap with nothing yet stale. Admission therefore evicts on demand, in
    /// least-recently-used order, and only workers the supervisor confirms are
    /// idle — evicting a busy one would kill live work to satisfy a memory
    /// limit, which is the trade this whole change exists to avoid making.
    async fn take_slot(&self) -> Result<tokio::sync::OwnedSemaphorePermit, RuntimeError> {
        if let Ok(slot) = Arc::clone(&self.slots).try_acquire_owned() {
            return Ok(slot);
        }
        let mut candidates: Vec<(WorkerKey, SupervisorHandle, std::time::Instant)> =
            self.workers.read().await.iter()
                .map(|(key, e)| (key.clone(), e.handle.clone(), e.last_used))
                .collect();
        candidates.sort_by_key(|(_, _, last_used)| *last_used);

        for (key, handle, _) in candidates.into_iter().take(MAX_EVICTION_PROBES) {
            if !handle.is_idle().await {
                continue;
            }
            // Dropping the entry is what returns its slot; claim the freed one
            // before another caller can. A worker re-used since the probe is no
            // longer under this key, and keeps its slot.
            let Some(entry) = self.workers.write().await.remove(&key) else { continue };
            drop(entry);
            let _ = handle.stop().await;
            if let Ok(slot) = Arc::clone(&self.slots).try_acquire_owned() {
                info!(app_id = %key.0, "evicted idle worker to admit a new one");
                return Ok(slot);
            }
        }

        let (_, cap) = self.pool_usage().await;
        Err(RuntimeError::Capacity(format!(
            "worker capacity reached ({cap} processes, all busy); retry shortly"
        )))
    }

    /// Route a unit of work to the worker bound to `(app_id, principal)`,
    /// spawning it on first use. The principal is set by the core here — never
    /// taken from a worker message — so a worker can only ever act as the one
    /// principal it was spawned for.
    async fn get_or_spawn(
        &self, app_id: &str, principal: Principal, invocation: InvocationContext,
    ) -> Result<SupervisorHandle, RuntimeError> {
        let key = worker_key(app_id, &principal, &invocation);
        // The read guard must drop at this semicolon: an `if let` scrutinee
        // temporary lives through the then-block, so reading inline would hold
        // the read lock across the write().await below — a self-deadlock that
        // wedges every later caller (tokio's RwLock queues readers behind a
        // waiting writer).
        let cached = self.workers.read().await.get(&key).cloned();
        if let Some(entry) = cached {
            if entry.handle.status().await? == WorkerStatus::Running {
                // Touch on every dispatch: the reaper's clock must measure time
                // since last USE, not since spawn.
                if let Some(e) = self.workers.write().await.get_mut(&key) {
                    e.last_used = std::time::Instant::now();
                }
                return Ok(entry.handle);
            }
            let _ = entry.handle.stop().await;
            self.workers.write().await.remove(&key);
        }
        // Admission before spawn: a process that cannot be accounted for must
        // never be started, or the cap is decorative.
        let slot = Arc::new(self.take_slot().await?);
        let handle = self
            .spawn_for(&self.pool, &self.secret_manager, app_id, &principal, &invocation)
            .await?;
        // Lost-race guard: another task may have spawned the same key meanwhile.
        let mut w = self.workers.write().await;
        if let Some(existing) = w.get(&key).cloned() {
            drop(w);
            let _ = handle.stop().await;
            return Ok(existing.handle);
        }
        w.insert(key, WorkerEntry { handle: handle.clone(), last_used: std::time::Instant::now(), _slot: slot });
        info!(app_id, "worker started");
        Ok(handle)
    }

    /// Start the per-app lifecycle (system) worker, which runs onStart. User and
    /// agent workers spawn lazily on first request. Shares the single per-identity
    /// spawn path; `pool`/`secrets` are vestigial (the manager holds its own).
    pub async fn start_app(&self, _pool: &PgPool, _secrets: &SecretManager, app_id: &str) -> Result<(), RuntimeError> {
        self.get_or_spawn(app_id, Principal::System, InvocationContext::default()).await.map(|_| ())
    }

    pub async fn stop_app(&self, app_id: &str) -> Result<(), RuntimeError> {
        let handles: Vec<(WorkerKey, SupervisorHandle)> = self.workers.read().await
            .iter().filter(|((a, _), _)| a == app_id).map(|(k, e)| (k.clone(), e.handle.clone())).collect();
        if handles.is_empty() { warn!(app_id, "no worker to stop"); return Ok(()); }
        for (key, h) in handles {
            let _ = h.stop().await;
            self.workers.write().await.remove(&key);
        }
        info!(app_id, "workers stopped");
        Ok(())
    }

    pub async fn start_deployed_apps(&self, pool: &PgPool, secrets: &SecretManager) {
        let Ok(entries) = std::fs::read_dir(&self.apps_dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() || resolve_entry_point(&path).is_err() { continue; }
            let app_id = entry.file_name().to_string_lossy().to_string();

            if let Some(def) = crate::extensions::agents::config::load_agent_json(&path).await {
                if let Err(e) = crate::extensions::agents::register_agent(pool, &app_id, &def, None).await {
                    error!(app_id = %app_id, "re-register agent: {e}");
                }
            }
            if let Err(e) = self.start_app(pool, secrets, &app_id).await {
                error!(app_id = %app_id, "auto-start failed: {e}");
            }
        }
    }

    pub async fn restart_all(&self, pool: &PgPool, secrets: &SecretManager) -> usize {
        let apps: std::collections::HashSet<String> =
            self.workers.read().await.keys().map(|(a, _)| a.clone()).collect();
        let count = apps.len();
        // Drop every worker (lifecycle + user + agent); user workers respawn
        // lazily with fresh creds, lifecycle workers are restarted here.
        self.stop_all(RESTART_GRACE).await;
        for app_id in &apps {
            if let Err(e) = self.start_app(pool, secrets, app_id).await { error!(app_id = %app_id, "restart start: {e}"); }
        }
        info!(count, "apps restarted (platform secrets changed)");
        count
    }

    /// Stop every worker, giving work already in flight `grace` to finish first.
    ///
    /// Stopping is a SIGKILL of the child, so without this a pod shutdown kills
    /// jobs mid-write: their pgmq lease lapses, the message redelivers, and the
    /// automation runs a second time. Every ordinary deploy did that. Waiting for
    /// the supervisor to report itself idle lets the work commit and archive its
    /// message, so there is nothing left to redeliver.
    ///
    /// Draining is polled from here rather than inside the supervisor because the
    /// supervisor must keep running to observe the completion it is waiting for.
    pub async fn stop_all(&self, grace: std::time::Duration) {
        let handles: Vec<(WorkerKey, SupervisorHandle)> =
            self.workers.read().await.iter().map(|(k, e)| (k.clone(), e.handle.clone())).collect();
        let deadline = tokio::time::Instant::now() + grace;
        let futs = handles.into_iter().map(|(key, h)| {
            let workers = Arc::clone(&self.workers);
            async move {
                drain_until_idle(&h, deadline).await;
                let _ = h.stop().await;
                workers.write().await.remove(&key);
            }
        });
        join_all(futs).await;
    }

    pub async fn invalidate_for_principal(&self, user_id: uuid::Uuid) {
        let affected: Vec<(WorkerKey, SupervisorHandle)> = self.workers.read().await.iter()
            .filter(|(key, _)| worker_key_belongs_to_principal(key, user_id))
            .map(|(key, e)| (key.clone(), e.handle.clone()))
            .collect();

        for (key, handle) in affected {
            info!(app_id = %key.0, %user_id, "invalidating worker (permission change)");
            if let Err(e) = handle.stop().await {
                error!(app_id = %key.0, "invalidate stop: {e}");
            }
            self.workers.write().await.remove(&key);
        }
    }

    /// Stop workers for all principals that hold a given role.
    /// Used when role permissions/inheritance change.
    pub async fn invalidate_for_role(&self, pool: &PgPool, role: &str) {
        let user_ids: Vec<(uuid::Uuid,)> = sqlx::query_as(
            "SELECT user_id FROM rootcx_system.rbac_assignments WHERE role = $1",
        ).bind(role).fetch_all(pool).await.unwrap_or_default();

        for (uid,) in user_ids {
            self.invalidate_for_principal(uid).await;
        }
    }

    pub async fn rpc(
        &self, app_id: &str, id: String, method: String, params: JsonValue, caller: Option<RpcCaller>,
    ) -> Result<JsonValue, RuntimeError> {
        let principal = Principal::from_request(crate::governance::enforcement::ContextState::from_caller(caller.as_ref()));
        self.get_or_spawn(app_id, principal, InvocationContext::default()).await?
            .rpc(id, method, params, caller).await
    }

    pub async fn rpc_action(
        &self, app_id: &str, id: String, action: String, params: JsonValue,
        caller: Option<RpcCaller>,
    ) -> Result<JsonValue, RuntimeError> {
        let principal = Principal::from_request(
            crate::governance::enforcement::ContextState::from_caller(caller.as_ref()),
        );
        let invocation = InvocationContext::action(&action);
        self.get_or_spawn(app_id, principal, invocation).await?
            .rpc(id, action, params, caller).await
    }

    /// Invoke an app's agent. `parent_perms` is the invoking parent agent's
    /// ALREADY-FROZEN effective set on a sub-invoke (`Some`), or `None` at the
    /// top of a run-tree (human / cron / webhook / channel). The child narrows
    /// against the parent, never re-widening against the human, so authority is
    /// monotone non-increasing down the chain.
    pub async fn agent_invoke(
        &self, app_id: &str, payload: AgentInvokePayload, parent_perms: Option<Vec<String>>,
    ) -> Result<mpsc::Receiver<AgentEvent>, RuntimeError> {
        // Freeze the delegated identity HERE so the worker is keyed and spawned
        // bound to exactly that authority. user_id stays the human (RLS row
        // ownership); effective_perms is the narrowed intersection.
        let agent_uid = crate::extensions::agents::agent_user_id(app_id);
        let effective_perms = crate::governance::authority::delegated_effective(
            &self.pool, agent_uid, payload.invoker_user_id, parent_perms.as_deref(),
        ).await;
        let identity = crate::governance::enforcement::ContextState {
            user_id: payload.invoker_user_id, is_delegated: true, effective_perms,
            connection_id: None,
            audit_actor_id: Some(agent_uid), audit_delegator_id: payload.invoker_user_id,
        };
        // An agent invoke is always a delegated principal, never anonymous.
        let session_id = payload.session_id.clone();
        let mut inner_rx = self.get_or_spawn(
            app_id, Principal::User(identity), InvocationContext::default(),
        ).await?.agent_invoke(payload).await?;

        // Fan out events to fleet broadcast for real-time monitoring
        let (outer_tx, outer_rx) = mpsc::channel(64);
        let fleet_tx = self.fleet_tx.clone();
        let app_id = app_id.to_string();
        tokio::spawn(async move {
            while let Some(event) = inner_rx.recv().await {
                let _ = fleet_tx.send(FleetEvent {
                    app_id: app_id.clone(),
                    session_id: session_id.clone(),
                    event: event.clone(),
                });
                if outer_tx.send(event).await.is_err() { break; }
            }
        });

        Ok(outer_rx)
    }

    pub fn subscribe_fleet(&self) -> broadcast::Receiver<FleetEvent> {
        self.fleet_tx.subscribe()
    }

    pub async fn dispatch_job(
        &self, app_id: &str, job_id: String, payload: JsonValue,
        caller: Option<RpcCaller>, cron_name: Option<&str>,
    ) -> Result<(), RuntimeError> {
        let principal = Principal::from_request(crate::governance::enforcement::ContextState::from_caller(caller.as_ref()));
        let invocation = cron_name
            .map(InvocationContext::job)
            .unwrap_or_default();
        self.get_or_spawn(app_id, principal, invocation).await?
            .dispatch_job(job_id, payload, caller).await
    }

    /// Aggregate status for an app across all its identity workers (Running if
    /// any worker is running).
    pub async fn worker_status(&self, app_id: &str) -> Result<WorkerStatus, RuntimeError> {
        let handles: Vec<SupervisorHandle> = self.workers.read().await
            .iter().filter(|((a, _), _)| a == app_id).map(|(_, e)| e.handle.clone()).collect();
        if handles.is_empty() { return Err(RuntimeError::Worker(format!("no worker for app '{app_id}'"))); }
        // Running wins; poll all identity workers concurrently.
        let mut agg = WorkerStatus::Stopped;
        for s in join_all(handles.iter().map(|h| h.status())).await.into_iter().flatten() {
            if s == WorkerStatus::Running { return Ok(WorkerStatus::Running); }
            agg = s;
        }
        Ok(agg)
    }

    pub async fn subscribe_logs(&self, app_id: &str) -> Result<broadcast::Receiver<LogEntry>, RuntimeError> {
        // Logs stream from the lifecycle worker. Per-identity worker log fan-in
        // is a known follow-up (see token-confusion fix notes).
        self.get_or_spawn(app_id, Principal::System, InvocationContext::default()).await.map(|h| h.subscribe())
    }

    /// Live worker processes and the pod's ceiling. Surfaced in `/health?full`
    /// because approaching the cap is the signal that a tenant needs a larger
    /// plan, and it is the only warning before callers start being refused.
    pub async fn pool_usage(&self) -> (usize, usize) {
        let live = self.workers.read().await.len();
        (live, live + self.slots.available_permits())
    }

    /// Aggregate per-app status across identity workers (Running wins).
    pub async fn all_statuses(&self) -> HashMap<String, WorkerStatus> {
        let handles: Vec<(String, SupervisorHandle)> =
            self.workers.read().await.iter().map(|((a, _), e)| (a.clone(), e.handle.clone())).collect();
        // Poll all workers concurrently, then fold per app (Running wins).
        let results = join_all(handles.into_iter().map(|(app, h)| async move { (app, h.status().await.ok()) })).await;
        let mut out: HashMap<String, WorkerStatus> = HashMap::new();
        for (app, s) in results.into_iter().filter_map(|(a, s)| s.map(|s| (a, s))) {
            out.entry(app)
                .and_modify(|cur| { if *cur != WorkerStatus::Running { *cur = s.clone(); } })
                .or_insert(s);
        }
        out
    }
}

fn apply_openai_base_url(
    credentials: &mut std::collections::HashMap<String, String>,
    configured_url: Option<String>,
) {
    let Some(url) = configured_url.map(|url| url.trim().to_string()) else {
        return;
    };
    if !url.is_empty() {
        credentials.entry("OPENAI_BASE_URL".into()).or_insert(url);
    }
}

#[cfg(test)]
mod openai_base_url_tests {
    use super::apply_openai_base_url;
    use std::collections::HashMap;

    #[test]
    fn chart_endpoint_reaches_sandboxed_agents() {
        let mut credentials = HashMap::new();

        apply_openai_base_url(
            &mut credentials,
            Some(" https://gateway.internal.example/v1/ ".into()),
        );

        assert_eq!(
            credentials.get("OPENAI_BASE_URL").map(String::as_str),
            Some("https://gateway.internal.example/v1/")
        );
    }

    #[test]
    fn app_specific_endpoint_takes_precedence() {
        let mut credentials = HashMap::from([(
            "OPENAI_BASE_URL".into(),
            "https://app-gateway.internal/v1".into(),
        )]);

        apply_openai_base_url(
            &mut credentials,
            Some("https://platform-gateway.internal/v1".into()),
        );

        assert_eq!(
            credentials.get("OPENAI_BASE_URL").map(String::as_str),
            Some("https://app-gateway.internal/v1")
        );
    }
}

// -- Sub-agent dispatch (implements AgentDispatcher for cross-worker invocation) --

struct SubAgentDispatch {
    wm: Arc<WorkerManager>,
}

#[async_trait]
impl AgentDispatcher for SubAgentDispatch {
    async fn dispatch(
        &self, pool: &PgPool, caller: &str, target: &str, message: &str,
        parent_tx: Option<mpsc::Sender<AgentEvent>>,
        invoker_user_id: Option<uuid::Uuid>,
        parent_perms: Vec<String>,
        task_scope: Option<Vec<String>>,
    ) -> Result<String, String> {
        if target == caller { return Err("cannot invoke self".into()); }

        let llm = crate::routes::llm_models::fetch_default_llm(pool).await
            .map_err(|e| e.to_string())?
            .map(|(provider, model)| LlmModelRef { provider, model });

        let payload = AgentInvokePayload {
            invoke_id: uuid::Uuid::new_v4().to_string(),
            session_id: uuid::Uuid::new_v4().to_string(),
            message: message.to_string(),
            history: vec![],
            is_sub_invoke: true,
            llm,
            invoker_user_id,
            attachments: None,
            task_scope,
        };

        let app_id = target.to_string();
        let mut rx = self.wm.agent_invoke(target, payload, Some(parent_perms)).await.map_err(|e| e.to_string())?;
        let mut response = String::new();
        while let Some(event) = rx.recv().await {
            match event {
                AgentEvent::Done { response: r, .. } => return Ok(r),
                AgentEvent::Error { error } => return Err(error),
                AgentEvent::Chunk { delta } => {
                    response.push_str(&delta);
                    if let Some(ref tx) = parent_tx {
                        let _ = tx.send(AgentEvent::SubAgentChunk { app_id: app_id.clone(), delta }).await;
                    }
                }
                AgentEvent::ApprovalRequired { .. } => {
                    if let Some(ref tx) = parent_tx {
                        let _ = tx.send(event).await;
                    }
                }
                _ => {}
            }
        }
        if response.is_empty() { Err("no response from agent".into()) } else { Ok(response) }
    }
}

// -- App action caller (executes app actions via worker RPC) --

struct AppActionCallImpl {
    wm: Arc<WorkerManager>,
}

#[async_trait]
impl ActionCaller for AppActionCallImpl {
    async fn call(
        &self, app_id: &str, action_id: &str, input: JsonValue, user_id: uuid::Uuid,
        _caller_app_id: &str, effective_perms: Option<Vec<String>>,
    ) -> Result<JsonValue, String> {
        // Phase 6a: the agent's effective authority (intersection grant∩human)
        // rides along on the caller so the target re-poses it as the RLS GUC.
        // No token: the worker never replays a JWT.
        let caller = Some(RpcCaller {
            user_id: user_id.to_string(),
            email: String::new(),
            effective_perms,
            connection_id: None,
        });
        self.wm.rpc_action(
            app_id,
            uuid::Uuid::new_v4().to_string(),
            action_id.to_string(),
            input,
            caller,
        ).await.map_err(|e| e.to_string())
    }
}

// -- Integration caller (executes integration actions via worker RPC) --

struct IntegrationCallImpl {
    wm: Arc<WorkerManager>,
    secrets: Arc<SecretManager>,
}

#[async_trait]
impl IntegrationCaller for IntegrationCallImpl {
    async fn call(
        &self, pool: &PgPool, user_id: uuid::Uuid, app_id: Option<&str>,
        integration_id: &str, action_id: &str, input: JsonValue,
        caller: Option<RpcCaller>,
    ) -> Result<JsonValue, String> {
        // If the caller carries a pinned connection_id (set by the fan-out or
        // inherited from a parent call), resolve against that connection directly
        // instead of the ORDER BY created_at fallback that picks the oldest.
        let pinned = caller.as_ref().and_then(|c| c.connection_id.as_deref());
        let (config, user_credentials, effective_uid, conn_id) = if let Some(cid) = pinned {
            let resolved = crate::extensions::integrations::connections::resolve_by_connection_id(
                &self.secrets, pool, integration_id, cid, &user_id.to_string(),
            ).await;
            if resolved.3.is_none() {
                tracing::warn!(integration_id, connection_id = cid, "pinned connection vanished or has no credentials");
            }
            resolved
        } else {
            crate::extensions::integrations::connections::resolve_credentials(
                &self.secrets, pool, integration_id, &user_id.to_string(), app_id,
            ).await
        };

        // `caller` is the RLS identity the sub-worker runs under; `effective_uid`
        // only selects whose mailbox/connection serves the request (mirrors the
        // HTTP action route). Passing the caller through is what keeps the worker
        // off the anonymous principal.
        let result = self.wm.rpc(
            integration_id,
            uuid::Uuid::new_v4().to_string(),
            "__integration".into(),
            serde_json::json!({
                "action": action_id, "input": input, "config": config,
                "userCredentials": user_credentials, "userId": effective_uid,
                "connectionId": conn_id,
            }),
            caller,
        ).await.map_err(|e| e.to_string())?;

        crate::extensions::integrations::connections::flag_if_auth_failed(pool, integration_id, conn_id.as_deref(), &result).await;
        Ok(result)
    }
}

fn resolve_entry_point(app_dir: &Path) -> Result<PathBuf, RuntimeError> {
    for name in ["index.ts", "index.js", "main.ts", "main.js", "src/index.ts", "src/index.js"] {
        let p = app_dir.join(name);
        if p.exists() { return Ok(p); }
    }
    Err(RuntimeError::Worker(format!("no entry point in {}", app_dir.display())))
}

#[cfg(test)]
mod tests {
    use super::{
        DRAIN_POLL, SupervisorHandle, drain_until_idle,
        DEFAULT_WORKER_CAP, MAX_WORKERS, MIN_WORKERS, Principal, worker_cap, worker_key,
        worker_key_belongs_to_principal,
    };
    use crate::governance::enforcement::{ContextState, InvocationContext};
    use uuid::Uuid;

    fn user(uid: Option<Uuid>, delegated: bool, perms: &[&str]) -> Principal {
        Principal::User(ContextState {
            user_id: uid,
            is_delegated: delegated,
            effective_perms: perms.iter().map(|s| s.to_string()).collect(),
            connection_id: None,
            audit_actor_id: uid,
            audit_delegator_id: None,
        })
    }

    // The worker-routing key for one User identity must be stable regardless of
    // perm ordering. A bug here (e.g. forgetting to sort) would spawn a fresh
    // worker per call (churn) instead of reusing.
    #[test]
    fn user_key_is_order_independent() {
        let u = Uuid::new_v4();
        assert_eq!(
            user(Some(u), true, &["b", "a", "c"]).key(),
            user(Some(u), true, &["c", "b", "a"]).key(),
            "permission order must not change the worker key",
        );
    }

    /// Isolation is opt-in per action now, but when an action asks for it the
    /// separation must be total: the scope is unforgeable only because no two
    /// scopes share a process.
    #[test]
    fn isolated_scopes_never_share_a_worker() {
        let principal = user(Some(Uuid::new_v4()), false, &[]);
        let scopes = [
            InvocationContext::default(),
            InvocationContext::action("approve"),
            InvocationContext::action("reject"),
            InvocationContext::job("stock-minimum-purchase-proposals"),
        ];
        for i in 0..scopes.len() {
            for j in (i + 1)..scopes.len() {
                assert_ne!(
                    worker_key("purchases", &principal, &scopes[i]),
                    worker_key("purchases", &principal, &scopes[j]),
                    "distinct Core execution scopes shared a worker",
                );
            }
        }
    }

    #[test]
    fn permission_invalidation_selects_every_worker_owned_by_the_principal() {
        let user_id = Uuid::new_v4();
        let agent_app = "agent-owned-by-principal";
        let agent_id = crate::extensions::agents::agent_user_id(agent_app);
        let cases = [
            (
                "direct human worker",
                ("app-a".to_string(), user(Some(user_id), false, &[]).key()),
                user_id,
                true,
            ),
            (
                "delegated human worker",
                ("app-b".to_string(), user(Some(user_id), true, &["app:x:read"]).key()),
                user_id,
                true,
            ),
            (
                "agent worker",
                (agent_app.to_string(), user(None, true, &[]).key()),
                agent_id,
                true,
            ),
            (
                "unrelated worker",
                ("app-c".to_string(), user(Some(Uuid::new_v4()), true, &[]).key()),
                user_id,
                false,
            ),
        ];
        for (label, key, principal, expected) in cases {
            assert_eq!(
                worker_key_belongs_to_principal(&key, principal),
                expected,
                "wrong invalidation decision for {label}",
            );
        }
    }

    // The security-critical property: distinct principals NEVER share a worker.
    // If two collided, one could act inside another's process. Crucially this
    // includes System vs Anonymous: untrusted anonymous traffic must never land
    // on the privileged onStart/BYPASSRLS worker.
    #[test]
    fn distinct_principals_never_share_a_worker() {
        let u1 = Uuid::new_v4();
        let u2 = Uuid::new_v4();
        let principals = [
            ("system", Principal::System),
            ("anonymous", Principal::Anonymous),
            ("u1 direct", user(Some(u1), false, &[])),
            ("u1 delegated", user(Some(u1), true, &["app:x:invoke"])),
            ("u1 direct, extra perm", user(Some(u1), false, &["app:x:invoke"])),
            ("u2 direct", user(Some(u2), false, &[])),
            ("no-user delegated", user(None, true, &[])),
        ];
        for i in 0..principals.len() {
            for j in (i + 1)..principals.len() {
                assert_ne!(
                    principals[i].1.key(), principals[j].1.key(),
                    "'{}' and '{}' must not share a worker", principals[i].0, principals[j].0,
                );
            }
        }
    }

    // Only System runs onStart / may BYPASSRLS. Anonymous (no-user requests) and
    // every User must not — else they would inherit the self-schema bypass.
    #[test]
    fn only_system_runs_onstart() {
        assert!(Principal::System.run_onstart());
        assert!(!Principal::Anonymous.run_onstart());
        assert!(!user(Some(Uuid::new_v4()), false, &[]).run_onstart());
        assert!(!user(None, true, &[]).run_onstart());
    }

    // A no-user request is Anonymous, NOT System: it gets its own worker, off the
    // privileged onStart process. (Regression guard for follow-up #1.)
    #[test]
    fn empty_request_identity_is_anonymous_not_system() {
        let p = Principal::from_request(ContextState::default());
        assert!(matches!(p, Principal::Anonymous));
        assert_eq!(p.key(), Principal::Anonymous.key());
        assert_ne!(p.key(), Principal::System.key());
        assert!(!p.run_onstart());
        // A real user is classified as User, never Anonymous/System.
        assert!(matches!(
            Principal::from_request(ContextState { user_id: Some(Uuid::new_v4()), is_delegated: false, effective_perms: vec![], connection_id: None, audit_actor_id: None, audit_delegator_id: None }),
            Principal::User(_)
        ));
        // A delegated no-user principal (cron/webhook agent) is a real authority,
        // NOT anonymous: it must get its own worker. Guards against simplifying
        // from_request to a `user_id.is_none()` check alone.
        assert!(matches!(
            Principal::from_request(ContextState { user_id: None, is_delegated: true, effective_perms: vec![], connection_id: None, audit_actor_id: None, audit_delegator_id: None }),
            Principal::User(_)
        ));
    }

    /// The cap has to come from the pod, not from a constant: the same binary
    /// serves a 512 MiB tenant and a 32 GiB one. Sized wrong in either direction
    /// it is useless — too high and the pod still OOMs, too low and ordinary use
    /// is refused.
    /// Draining must yield the moment a worker reports idle, and must not wait
    /// forever on one that never will. Both halves matter: returning early on a
    /// busy worker kills work mid-write (the duplicate-automation bug), and
    /// never returning wedges the pod past the kubelet's grace into a SIGKILL,
    /// which is the same bug with extra steps.
    #[tokio::test(start_paused = true)]
    async fn draining_waits_for_a_busy_worker_but_not_past_the_deadline() {
        let grace = std::time::Duration::from_secs(20);

        let idle = SupervisorHandle::stub(true);
        let started = tokio::time::Instant::now();
        drain_until_idle(&idle, started + grace).await;
        assert!(
            started.elapsed() < DRAIN_POLL,
            "an idle worker must be stopped at once, not held for the whole grace",
        );

        let busy = SupervisorHandle::stub(false);
        let started = tokio::time::Instant::now();
        // Bounded, so a drain that never gives up fails the test instead of
        // hanging it: a hung test reports as a CI timeout with no explanation.
        tokio::time::timeout(grace * 2, drain_until_idle(&busy, started + grace))
            .await
            .expect("draining must end at the deadline, not wait on a worker that never finishes");
        assert!(
            started.elapsed() >= grace,
            "a busy worker must be given the whole grace before it is killed",
        );
    }

    #[test]
    fn the_cap_follows_the_pod_and_never_leaves_its_bounds() {
        let mib = 1024 * 1024;

        // The default tenant plan. Small on purpose: this is the pod that a
        // single user exercising a dozen actions used to kill.
        let micro = worker_cap(Some(512 * mib), None, DEFAULT_WORKER_CAP);
        assert!(
            (MIN_WORKERS..=8).contains(&micro),
            "512 MiB must yield a handful of workers, got {micro}",
        );
        assert!(
            worker_cap(Some(8192 * mib), None, DEFAULT_WORKER_CAP) > micro,
            "a larger pod must host more workers",
        );

        assert_eq!(
            worker_cap(Some(64 * mib), None, DEFAULT_WORKER_CAP),
            MIN_WORKERS,
            "a pod too small to budget for must still host a lifecycle worker and a caller",
        );
        assert_eq!(
            worker_cap(Some(u64::MAX), None, DEFAULT_WORKER_CAP),
            MAX_WORKERS,
            "an unbounded cgroup must not produce an unbounded cap",
        );
        assert_eq!(
            worker_cap(None, None, DEFAULT_WORKER_CAP),
            DEFAULT_WORKER_CAP,
            "off cgroup (a developer machine) falls back rather than guessing",
        );
    }

    #[test]
    fn an_operator_override_wins_but_is_still_clamped() {
        assert_eq!(worker_cap(Some(512 * 1024 * 1024), Some("40"), DEFAULT_WORKER_CAP), 40);
        assert_eq!(worker_cap(None, Some(" 40 "), DEFAULT_WORKER_CAP), 40);
        assert_eq!(
            worker_cap(None, Some("0"), DEFAULT_WORKER_CAP), MIN_WORKERS,
            "zero would wedge the runtime shut; clamp rather than obey",
        );
        assert_eq!(
            worker_cap(None, Some("nonsense"), DEFAULT_WORKER_CAP), DEFAULT_WORKER_CAP,
            "an unparseable override must not silently disable the cap",
        );
    }
}
