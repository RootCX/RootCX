use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use futures::future::join_all;
use serde_json::Value as JsonValue;
use sqlx::PgPool;
use tokio::sync::{RwLock, broadcast, mpsc};
use tracing::{error, info, warn};

use crate::RuntimeError;
use crate::extensions::agents::approvals::PendingApprovals;
use crate::extensions::logs::LogEntry;
use crate::ipc::{AgentBootConfig, AgentInvokePayload, LlmModelRef, RpcCaller};
use crate::secrets::SecretManager;
use crate::tools::{ActionCaller, AgentDispatcher, IntegrationCaller, ToolRegistry};
use crate::worker::{self, AgentEvent, FleetEvent, SupervisorHandle, WorkerConfig, WorkerStatus};

const BACKEND_PRELUDE: &str = include_str!("backend_prelude.js");

/// A worker process is keyed by (app_id, identity). One process serves exactly
/// ONE identity for its whole life, so a malicious app can never act as another
/// user (cross-user confused deputy is structurally impossible) and there is no
/// token to forge. See docs/security-context-token-confusion.md.
type WorkerKey = (String, String);

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
    User(crate::sql_proxy::ContextState),
}

impl Principal {
    /// Classify the identity resolved for an incoming request. A request never
    /// yields System; an empty identity (no user, not delegated) is Anonymous.
    fn from_request(state: crate::sql_proxy::ContextState) -> Self {
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
    fn rls_state(&self) -> crate::sql_proxy::ContextState {
        match self {
            Principal::User(s) => s.clone(),
            _ => crate::sql_proxy::ContextState::default(),
        }
    }
}

pub struct WorkerManager {
    workers: Arc<RwLock<HashMap<WorkerKey, SupervisorHandle>>>,
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
    ) -> Self {
        let prelude_path = apps_dir.join(".prelude.js");
        std::fs::write(&prelude_path, BACKEND_PRELUDE).expect("write backend prelude");
        let (fleet_tx, _) = broadcast::channel(512);
        Self {
            workers: Arc::new(RwLock::new(HashMap::new())),
            pool,
            dispatch: OnceLock::new(),
            action_call: OnceLock::new(),
            integration_call: OnceLock::new(),
            fleet_tx,
            apps_dir, prelude_path, runtime_url, bun_bin,
            tool_registry, pending_approvals, secret_manager, upload_nonces,
        }
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
            crate::extensions::rbac::policy::resolve_permissions(pool, agent_uid),
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
        principal: &Principal,
    ) -> Result<SupervisorHandle, RuntimeError> {
        let app_dir = self.apps_dir.join(app_id);
        let entry_point = resolve_entry_point(&app_dir)?;
        let credentials = secrets.get_env_for_app(pool, app_id).await?;
        let (agent_boot_config, supervision) = match self.build_agent_boot(pool, app_id).await {
            Some((boot, sup)) => (Some(boot), sup),
            None => (None, None),
        };
        let config = WorkerConfig {
            app_id: app_id.to_string(),
            identity: principal.rls_state(),
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
        let handle = worker::spawn_supervisor(config);
        handle.start().await?;
        Ok(handle)
    }

    /// Route a unit of work to the worker bound to `(app_id, principal)`,
    /// spawning it on first use. The principal is set by the core here — never
    /// taken from a worker message — so a worker can only ever act as the one
    /// principal it was spawned for.
    async fn get_or_spawn(
        &self, app_id: &str, principal: Principal,
    ) -> Result<SupervisorHandle, RuntimeError> {
        let key = (app_id.to_string(), principal.key());
        if let Some(h) = self.workers.read().await.get(&key).cloned() {
            if h.status().await? == WorkerStatus::Running { return Ok(h); }
            self.workers.write().await.remove(&key);
        }
        let handle = self.spawn_for(&self.pool, &self.secret_manager, app_id, &principal).await?;
        // Lost-race guard: another task may have spawned the same key meanwhile.
        let mut w = self.workers.write().await;
        if let Some(existing) = w.get(&key).cloned() {
            drop(w);
            let _ = handle.stop().await;
            return Ok(existing);
        }
        w.insert(key, handle.clone());
        info!(app_id, "worker started");
        Ok(handle)
    }

    /// Start the per-app lifecycle (system) worker, which runs onStart. User and
    /// agent workers spawn lazily on first request. Shares the single per-identity
    /// spawn path; `pool`/`secrets` are vestigial (the manager holds its own).
    pub async fn start_app(&self, _pool: &PgPool, _secrets: &SecretManager, app_id: &str) -> Result<(), RuntimeError> {
        self.get_or_spawn(app_id, Principal::System).await.map(|_| ())
    }

    pub async fn stop_app(&self, app_id: &str) -> Result<(), RuntimeError> {
        let handles: Vec<(WorkerKey, SupervisorHandle)> = self.workers.read().await
            .iter().filter(|((a, _), _)| a == app_id).map(|(k, h)| (k.clone(), h.clone())).collect();
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
        self.stop_all().await;
        for app_id in &apps {
            if let Err(e) = self.start_app(pool, secrets, app_id).await { error!(app_id = %app_id, "restart start: {e}"); }
        }
        info!(count, "apps restarted (platform secrets changed)");
        count
    }

    pub async fn stop_all(&self) {
        let handles: Vec<(WorkerKey, SupervisorHandle)> =
            self.workers.read().await.iter().map(|(k, h)| (k.clone(), h.clone())).collect();
        let futs = handles.into_iter().map(|(key, h)| {
            let workers = Arc::clone(&self.workers);
            async move { let _ = h.stop().await; workers.write().await.remove(&key); }
        });
        join_all(futs).await;
    }

    pub async fn invalidate_for_principal(&self, user_id: uuid::Uuid) {
        let target = self.workers.read().await.keys()
            .find(|(app_id, _)| crate::extensions::agents::agent_user_id(app_id) == user_id)
            .map(|(app_id, _)| app_id.clone());
        if let Some(app_id) = target {
            info!(app_id = %app_id, %user_id, "invalidating worker (permission change)");
            if let Err(e) = self.stop_app(&app_id).await {
                error!(app_id = %app_id, "invalidate stop: {e}");
            }
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
        let principal = Principal::from_request(crate::sql_proxy::ContextState::from_caller(caller.as_ref()));
        self.get_or_spawn(app_id, principal).await?.rpc(id, method, params, caller).await
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
        let effective_perms = crate::extensions::rbac::policy::delegated_effective(
            &self.pool, agent_uid, payload.invoker_user_id, parent_perms.as_deref(),
        ).await;
        let identity = crate::sql_proxy::ContextState {
            user_id: payload.invoker_user_id, is_delegated: true, effective_perms,
        };
        // An agent invoke is always a delegated principal, never anonymous.
        let session_id = payload.session_id.clone();
        let mut inner_rx = self.get_or_spawn(app_id, Principal::User(identity)).await?.agent_invoke(payload).await?;

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

    pub async fn dispatch_job(&self, app_id: &str, job_id: String, payload: JsonValue, caller: Option<RpcCaller>) -> Result<(), RuntimeError> {
        let principal = Principal::from_request(crate::sql_proxy::ContextState::from_caller(caller.as_ref()));
        self.get_or_spawn(app_id, principal).await?.dispatch_job(job_id, payload, caller).await
    }

    /// Aggregate status for an app across all its identity workers (Running if
    /// any worker is running).
    pub async fn worker_status(&self, app_id: &str) -> Result<WorkerStatus, RuntimeError> {
        let handles: Vec<SupervisorHandle> = self.workers.read().await
            .iter().filter(|((a, _), _)| a == app_id).map(|(_, h)| h.clone()).collect();
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
        self.get_or_spawn(app_id, Principal::System).await.map(|h| h.subscribe())
    }

    /// Aggregate per-app status across identity workers (Running wins).
    pub async fn all_statuses(&self) -> HashMap<String, WorkerStatus> {
        let handles: Vec<(String, SupervisorHandle)> =
            self.workers.read().await.iter().map(|((a, _), h)| (a.clone(), h.clone())).collect();
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
        });
        self.wm.rpc(
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
        &self, pool: &PgPool, user_id: uuid::Uuid,
        integration_id: &str, action_id: &str, input: JsonValue,
    ) -> Result<JsonValue, String> {
        let config = crate::extensions::integrations::routes::resolve_config(pool, &self.secrets, integration_id)
            .await.map_err(|e| format!("{e:?}"))?;

        let (user_credentials, effective_uid) = crate::extensions::integrations::connections::resolve_credentials(
            &self.secrets, pool, integration_id, &user_id.to_string(), None,
        ).await;

        self.wm.rpc(
            integration_id,
            uuid::Uuid::new_v4().to_string(),
            "__integration".into(),
            serde_json::json!({
                "action": action_id, "input": input, "config": config,
                "userCredentials": user_credentials, "userId": effective_uid,
            }),
            None,
        ).await.map_err(|e| e.to_string())
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
    use super::Principal;
    use crate::sql_proxy::ContextState;
    use uuid::Uuid;

    fn user(uid: Option<Uuid>, delegated: bool, perms: &[&str]) -> Principal {
        Principal::User(ContextState {
            user_id: uid,
            is_delegated: delegated,
            effective_perms: perms.iter().map(|s| s.to_string()).collect(),
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
            Principal::from_request(ContextState { user_id: Some(Uuid::new_v4()), is_delegated: false, effective_perms: vec![] }),
            Principal::User(_)
        ));
        // A delegated no-user principal (cron/webhook agent) is a real authority,
        // NOT anonymous: it must get its own worker. Guards against simplifying
        // from_request to a `user_id.is_none()` check alone.
        assert!(matches!(
            Principal::from_request(ContextState { user_id: None, is_delegated: true, effective_perms: vec![] }),
            Principal::User(_)
        ));
    }
}
