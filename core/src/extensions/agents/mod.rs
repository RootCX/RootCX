pub(crate) mod approvals;
pub(crate) mod config;
pub(crate) mod persistence;
pub(crate) mod routes;
pub(crate) mod streaming;
pub(crate) mod supervision;

use async_trait::async_trait;
use axum::Router;
use axum::routing::{get, patch, post};
use sqlx::PgPool;
use tracing::info;
use uuid::Uuid;

use super::RuntimeExtension;
use crate::RuntimeError;
use crate::routes::SharedRuntime;
use rootcx_types::{AgentDefinition, AppManifest};

/// Do not change — existing agent user_ids in the DB depend on this value.
const AGENT_UUID_NAMESPACE: Uuid = Uuid::from_bytes([
    0x9a, 0x3b, 0x4c, 0x5d, 0x6e, 0x7f, 0x40, 0x01,
    0x82, 0x93, 0xa4, 0xb5, 0xc6, 0xd7, 0xe8, 0xf9,
]);

pub fn agent_user_id(app_id: &str) -> Uuid {
    Uuid::new_v5(&AGENT_UUID_NAMESPACE, format!("agent:{app_id}").as_bytes())
}

pub struct AgentExtension;

#[async_trait]
impl RuntimeExtension for AgentExtension {
    fn name(&self) -> &str {
        "agents"
    }

    async fn bootstrap(&self, pool: &PgPool) -> Result<(), RuntimeError> {
        info!("bootstrapping agents extension");
        for ddl in [
            "CREATE TABLE IF NOT EXISTS rootcx_system.agents (
                app_id      TEXT PRIMARY KEY REFERENCES rootcx_system.apps(id) ON DELETE CASCADE,
                name        TEXT NOT NULL,
                description TEXT,
                config      JSONB NOT NULL DEFAULT '{}',
                created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
                updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
            )",
            "CREATE TABLE IF NOT EXISTS rootcx_system.agent_sessions (
                id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                app_id      TEXT NOT NULL REFERENCES rootcx_system.agents(app_id) ON DELETE CASCADE,
                user_id     UUID,
                messages    JSONB NOT NULL DEFAULT '[]',
                created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
                updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
            )",
            "CREATE INDEX IF NOT EXISTS idx_agent_sessions_app
                ON rootcx_system.agent_sessions (app_id)",
            "CREATE INDEX IF NOT EXISTS idx_agent_sessions_user
                ON rootcx_system.agent_sessions (user_id)",
            "ALTER TABLE rootcx_system.agent_sessions ADD COLUMN IF NOT EXISTS title TEXT",
            "ALTER TABLE rootcx_system.agent_sessions ADD COLUMN IF NOT EXISTS status TEXT NOT NULL DEFAULT 'active'",
            "ALTER TABLE rootcx_system.agent_sessions ADD COLUMN IF NOT EXISTS total_tokens BIGINT DEFAULT 0",
            "ALTER TABLE rootcx_system.agent_sessions ADD COLUMN IF NOT EXISTS turn_count INTEGER DEFAULT 0",
            "CREATE TABLE IF NOT EXISTS rootcx_system.agent_messages (
                id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                session_id  UUID NOT NULL REFERENCES rootcx_system.agent_sessions(id) ON DELETE CASCADE,
                role        TEXT NOT NULL,
                content     TEXT,
                token_count INTEGER DEFAULT 0,
                is_summary  BOOLEAN NOT NULL DEFAULT false,
                created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
            )",
            "CREATE INDEX IF NOT EXISTS idx_agent_messages_session
                ON rootcx_system.agent_messages (session_id, created_at)",
            "CREATE TABLE IF NOT EXISTS rootcx_system.agent_tool_calls (
                id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                session_id  UUID NOT NULL REFERENCES rootcx_system.agent_sessions(id) ON DELETE CASCADE,
                message_id  UUID REFERENCES rootcx_system.agent_messages(id) ON DELETE SET NULL,
                tool_name   TEXT NOT NULL,
                input       JSONB NOT NULL,
                output      JSONB,
                error       TEXT,
                status      TEXT NOT NULL DEFAULT 'pending',
                duration_ms INTEGER,
                created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
            )",
            "CREATE INDEX IF NOT EXISTS idx_agent_tool_calls_session
                ON rootcx_system.agent_tool_calls (session_id, created_at)",
        ] {
            sqlx::query(ddl).execute(pool).await.map_err(RuntimeError::Schema)?;
        }

        info!("agents extension ready");
        Ok(())
    }

    async fn on_app_installed(&self, pool: &PgPool, manifest: &AppManifest, _installed_by: uuid::Uuid) -> Result<(), RuntimeError> {
        let app_id = &manifest.app_id;
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM rootcx_system.agents WHERE app_id = $1)",
        )
        .bind(app_id)
        .fetch_one(pool)
        .await
        .map_err(RuntimeError::Schema)?;

        if !exists { return Ok(()); }

        // Ensure the agent system user exists but don't overwrite an existing
        // role assignment (register_agent is the authoritative path for permissions)
        let agent_uid = agent_user_id(app_id);
        sqlx::query(
            "INSERT INTO rootcx_system.users (id, email, is_system) \
             VALUES ($1, $2, true) ON CONFLICT (id) DO NOTHING"
        ).bind(agent_uid).bind(format!("agent+{app_id}@localhost"))
        .execute(pool).await.map_err(RuntimeError::Schema)?;

        let has_role: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM rootcx_system.rbac_assignments WHERE user_id = $1)"
        ).bind(agent_uid).fetch_one(pool).await.map_err(RuntimeError::Schema)?;

        if !has_role {
            sqlx::query(
                "INSERT INTO rootcx_system.rbac_assignments (user_id, role) \
                 VALUES ($1, 'admin') ON CONFLICT DO NOTHING"
            ).bind(agent_uid).execute(pool).await.map_err(RuntimeError::Schema)?;
        }
        info!(app = %app_id, "agent RBAC synced");
        Ok(())
    }

    fn routes(&self) -> Option<Router<SharedRuntime>> {
        Some(
            Router::new()
                .route("/api/v1/agents", get(routes::list_agents))
                .route("/api/v1/agents/{app_id}", patch(routes::update_agent).delete(routes::delete_agent))
                .route("/api/v1/apps/{app_id}/agent", get(routes::get_agent))
                .route("/api/v1/apps/{app_id}/agent/invoke", post(routes::invoke_agent))
                .route("/api/v1/apps/{app_id}/agent/sessions", get(routes::list_sessions))
                .route("/api/v1/apps/{app_id}/agent/sessions/{session_id}", get(routes::get_session))
                .route("/api/v1/apps/{app_id}/agent/sessions/{session_id}/events", get(routes::get_session_events))
                .route("/api/v1/apps/{app_id}/agent/approvals", get(routes::list_approvals))
                .route("/api/v1/apps/{app_id}/agent/approvals/{approval_id}", post(routes::reply_approval))
                .route("/api/v1/agents/stream", get(routes::fleet_stream)),
        )
    }
}

pub async fn register_agent(pool: &PgPool, app_id: &str, def: &AgentDefinition) -> Result<(), RuntimeError> {
    let config = serde_json::json!({
        "systemPrompt": def.system_prompt,
        "memory": def.memory,
        "limits": def.limits,
        "supervision": def.supervision,
    });

    sqlx::query(
        "INSERT INTO rootcx_system.agents (app_id, name, description, config)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (app_id) DO UPDATE SET
             name = EXCLUDED.name, description = EXCLUDED.description,
             config = EXCLUDED.config, updated_at = now()"
    )
    .bind(app_id)
    .bind(&def.name)
    .bind(def.description.as_deref())
    .bind(&config)
    .execute(pool)
    .await
    .map_err(RuntimeError::Schema)?;

    sync_agent_rbac(pool, app_id, def.permissions.as_deref()).await?;
    info!(app = %app_id, "agent registered from agent.json");
    Ok(())
}

/// Sync the agent's RBAC identity. If `grant` is provided, the agent gets
/// exactly those permissions (least-privilege). If None, falls back to `admin`
/// for backward compatibility with agents that don't declare permissions.
async fn sync_agent_rbac(pool: &PgPool, app_id: &str, grant: Option<&[String]>) -> Result<(), RuntimeError> {
    let agent_uid = agent_user_id(app_id);
    let email = format!("agent+{app_id}@localhost");

    // Ensure the system user exists
    sqlx::query(
        "INSERT INTO rootcx_system.users (id, email, is_system) \
         VALUES ($1, $2, true) ON CONFLICT (id) DO NOTHING"
    ).bind(agent_uid).bind(&email).execute(pool).await.map_err(RuntimeError::Schema)?;

    let role_name = match grant {
        None => {
            // No explicit grant: clean slate + assign admin (backward-compatible)
            sqlx::query("DELETE FROM rootcx_system.rbac_assignments WHERE user_id = $1")
                .bind(agent_uid).execute(pool).await.map_err(RuntimeError::Schema)?;
            sqlx::query(
                "INSERT INTO rootcx_system.rbac_assignments (user_id, role) \
                 VALUES ($1, 'admin') ON CONFLICT (user_id, role) DO NOTHING"
            ).bind(agent_uid).execute(pool).await.map_err(RuntimeError::Schema)?;
            return Ok(());
        }
        Some(perms) => {
            // Explicit grant: create/update a dedicated role for this agent
            let name = format!("agent:{app_id}");
            let perm_list: Vec<String> = perms.to_vec();
            sqlx::query(
                "INSERT INTO rootcx_system.rbac_roles (name, description, inherits, permissions) \
                 VALUES ($1, $2, '{}', $3) \
                 ON CONFLICT (name) DO UPDATE SET permissions = EXCLUDED.permissions"
            )
            .bind(&name)
            .bind(format!("Auto-generated grant for agent {app_id}"))
            .bind(&perm_list)
            .execute(pool).await.map_err(RuntimeError::Schema)?;
            name
        }
    };

    // Remove any previous role assignments and assign the agent role
    sqlx::query("DELETE FROM rootcx_system.rbac_assignments WHERE user_id = $1")
        .bind(agent_uid).execute(pool).await.map_err(RuntimeError::Schema)?;
    sqlx::query(
        "INSERT INTO rootcx_system.rbac_assignments (user_id, role) \
         VALUES ($1, $2) ON CONFLICT (user_id, role) DO NOTHING"
    ).bind(agent_uid).bind(&role_name).execute(pool).await.map_err(RuntimeError::Schema)?;

    Ok(())
}
