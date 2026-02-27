pub(crate) mod approvals;
pub(crate) mod config;
pub(crate) mod persistence;
pub(crate) mod routes;
pub(crate) mod streaming;
pub(crate) mod supervision;

use async_trait::async_trait;
use axum::Router;
use axum::routing::{get, post};
use sqlx::PgPool;
use tracing::info;
use uuid::Uuid;

use super::RuntimeExtension;
use crate::RuntimeError;
use crate::routes::SharedRuntime;
use rootcx_shared_types::AppManifest;

/// Fixed namespace for UUID v5 generation. Ensures `agent_user_id("my-app")`
/// always returns the same UUID, so agents keep their identity across restarts.
/// Do not change — existing agent user_ids in the DB depend on this value.
const AGENT_UUID_NAMESPACE: Uuid = Uuid::from_bytes([
    0x9a, 0x3b, 0x4c, 0x5d, 0x6e, 0x7f, 0x40, 0x01,
    0x82, 0x93, 0xa4, 0xb5, 0xc6, 0xd7, 0xe8, 0xf9,
]);

/// Deterministic user ID for an agent: same app_id always yields the same UUID.
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

    async fn on_app_installed(&self, pool: &PgPool, manifest: &AppManifest, _installed_by: uuid::Uuid, _tool_names: &[String]) -> Result<(), RuntimeError> {
        let def = match &manifest.agent {
            Some(d) => d,
            None => return Ok(()),
        };
        let app_id = &manifest.app_id;
        let role_name = format!("agent:{app_id}");
        let agent_uid = agent_user_id(app_id);

        let config = serde_json::json!({
            "provider": def.provider,
            "systemPrompt": def.system_prompt,
            "graph": def.graph,
            "memory": def.memory,
            "limits": def.limits,
            "supervision": def.supervision,
        });

        // All catalogued permissions — RbacExtension already derived them from dataContract
        let agent_permissions: Vec<String> = sqlx::query_scalar(
            "SELECT key FROM rootcx_system.rbac_permissions WHERE app_id = $1",
        )
        .bind(app_id)
        .fetch_all(pool)
        .await
        .map_err(RuntimeError::Schema)?;

        let mut tx = pool.begin().await.map_err(RuntimeError::Schema)?;

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
        .execute(&mut *tx)
        .await
        .map_err(RuntimeError::Schema)?;

        sqlx::query(
            "INSERT INTO rootcx_system.rbac_roles (app_id, name, description, inherits, permissions)
             VALUES ($1, $2, $3, '{}', $4)
             ON CONFLICT (app_id, name) DO UPDATE SET description = EXCLUDED.description, permissions = EXCLUDED.permissions"
        )
        .bind(app_id)
        .bind(&role_name)
        .bind(format!("Auto-created role for agent {}", def.name))
        .bind(&agent_permissions)
        .execute(&mut *tx)
        .await
        .map_err(RuntimeError::Schema)?;

        sqlx::query(
            "INSERT INTO rootcx_system.users (id, username, is_system)
             VALUES ($1, $2, true)
             ON CONFLICT (id) DO NOTHING"
        )
        .bind(agent_uid)
        .bind(&role_name)
        .execute(&mut *tx)
        .await
        .map_err(RuntimeError::Schema)?;

        sqlx::query(
            "INSERT INTO rootcx_system.rbac_assignments (user_id, app_id, role)
             VALUES ($1, $2, $3)
             ON CONFLICT (user_id, app_id, role) DO NOTHING"
        )
        .bind(agent_uid)
        .bind(app_id)
        .bind(&role_name)
        .execute(&mut *tx)
        .await
        .map_err(RuntimeError::Schema)?;

        tx.commit().await.map_err(RuntimeError::Schema)?;
        info!(app = %app_id, "agent registered");
        Ok(())
    }

    fn routes(&self) -> Option<Router<SharedRuntime>> {
        Some(
            Router::new()
                .route("/api/v1/apps/{app_id}/agent", get(routes::get_agent))
                .route("/api/v1/apps/{app_id}/agent/invoke", post(routes::invoke_agent))
                .route("/api/v1/apps/{app_id}/agent/sessions", get(routes::list_sessions))
                .route("/api/v1/apps/{app_id}/agent/sessions/{session_id}", get(routes::get_session))
                .route("/api/v1/apps/{app_id}/agent/sessions/{session_id}/events", get(routes::get_session_events))
                .route("/api/v1/apps/{app_id}/agent/approvals", get(routes::list_approvals))
                .route("/api/v1/apps/{app_id}/agent/approvals/{approval_id}", post(routes::reply_approval)),
        )
    }
}
