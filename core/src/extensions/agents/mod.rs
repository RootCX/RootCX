pub(crate) mod routes;

use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;
use axum::routing::{get, post};
use sqlx::PgPool;
use tracing::info;

use super::RuntimeExtension;
use crate::RuntimeError;
use crate::extensions::rbac::PolicyCache;
use crate::routes::SharedRuntime;
use rootcx_shared_types::AppManifest;

pub struct AgentExtension {
    rbac_cache: Arc<PolicyCache>,
}

impl AgentExtension {
    pub fn with_cache(rbac_cache: Arc<PolicyCache>) -> Self {
        Self { rbac_cache }
    }
}

#[async_trait]
impl RuntimeExtension for AgentExtension {
    fn name(&self) -> &str {
        "agents"
    }

    async fn bootstrap(&self, pool: &PgPool) -> Result<(), RuntimeError> {
        info!("bootstrapping agents extension");
        for ddl in [
            "CREATE TABLE IF NOT EXISTS rootcx_system.agents (
                id          TEXT NOT NULL,
                app_id      TEXT NOT NULL REFERENCES rootcx_system.apps(id) ON DELETE CASCADE,
                name        TEXT NOT NULL,
                description TEXT,
                model       TEXT,
                config      JSONB NOT NULL DEFAULT '{}',
                created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
                updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
                PRIMARY KEY (app_id, id)
            )",
            "CREATE TABLE IF NOT EXISTS rootcx_system.agent_sessions (
                id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                app_id      TEXT NOT NULL,
                agent_id    TEXT NOT NULL,
                user_id     UUID,
                messages    JSONB NOT NULL DEFAULT '[]',
                created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
                updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
                FOREIGN KEY (app_id, agent_id) REFERENCES rootcx_system.agents(app_id, id) ON DELETE CASCADE
            )",
            "CREATE INDEX IF NOT EXISTS idx_agent_sessions_agent
                ON rootcx_system.agent_sessions (app_id, agent_id)",
            "CREATE INDEX IF NOT EXISTS idx_agent_sessions_user
                ON rootcx_system.agent_sessions (user_id)",
        ] {
            sqlx::query(ddl).execute(pool).await.map_err(RuntimeError::Schema)?;
        }
        info!("agents extension ready");
        Ok(())
    }

    async fn on_app_installed(&self, pool: &PgPool, manifest: &AppManifest) -> Result<(), RuntimeError> {
        if manifest.agents.is_empty() {
            return Ok(());
        }
        let app_id = &manifest.app_id;

        for (agent_id, def) in &manifest.agents {
            let config = serde_json::json!({
                "systemPrompt": def.system_prompt,
                "memory": def.memory,
                "limits": def.limits,
                "access": def.access,
            });
            sqlx::query(
                "INSERT INTO rootcx_system.agents (id, app_id, name, description, model, config)
                 VALUES ($1, $2, $3, $4, $5, $6)
                 ON CONFLICT (app_id, id) DO UPDATE SET
                     name = EXCLUDED.name, description = EXCLUDED.description,
                     model = EXCLUDED.model, config = EXCLUDED.config, updated_at = now()"
            )
            .bind(agent_id)
            .bind(app_id)
            .bind(&def.name)
            .bind(def.description.as_deref())
            .bind(def.model.as_deref())
            .bind(&config)
            .execute(pool)
            .await
            .map_err(RuntimeError::Schema)?;

            let role_name = format!("agent:{agent_id}");
            sqlx::query(
                "INSERT INTO rootcx_system.rbac_roles (app_id, name, description, inherits)
                 VALUES ($1, $2, $3, '{}')
                 ON CONFLICT (app_id, name) DO UPDATE SET description = EXCLUDED.description"
            )
            .bind(app_id)
            .bind(&role_name)
            .bind(format!("Auto-created role for agent {}", def.name))
            .execute(pool)
            .await
            .map_err(RuntimeError::Schema)?;

            sqlx::query("DELETE FROM rootcx_system.rbac_policies WHERE app_id = $1 AND role = $2")
                .bind(app_id)
                .bind(&role_name)
                .execute(pool)
                .await
                .map_err(RuntimeError::Schema)?;

            for entry in &def.access {
                if entry.entity.starts_with("tool:") {
                    continue;
                }
                let actions: Vec<&str> = entry.actions.iter().map(String::as_str).collect();
                sqlx::query(
                    "INSERT INTO rootcx_system.rbac_policies (app_id, role, entity, actions, ownership)
                     VALUES ($1, $2, $3, $4, false)"
                )
                .bind(app_id)
                .bind(&role_name)
                .bind(&entry.entity)
                .bind(&actions)
                .execute(pool)
                .await
                .map_err(RuntimeError::Schema)?;
            }

            info!(app = %app_id, agent = %agent_id, "agent registered with RBAC");
        }

        let current_ids: Vec<&str> = manifest.agents.keys().map(String::as_str).collect();
        sqlx::query("DELETE FROM rootcx_system.agents WHERE app_id = $1 AND NOT (id = ANY($2))")
            .bind(app_id)
            .bind(&current_ids)
            .execute(pool)
            .await
            .map_err(RuntimeError::Schema)?;

        self.rbac_cache.invalidate(app_id);
        Ok(())
    }

    fn routes(&self) -> Option<Router<SharedRuntime>> {
        Some(
            Router::new()
                .route("/api/v1/apps/{app_id}/agents", get(routes::list_agents))
                .route(
                    "/api/v1/apps/{app_id}/agents/{agent_id}/invoke",
                    post(routes::invoke_agent),
                )
                .route(
                    "/api/v1/apps/{app_id}/agents/{agent_id}/sessions",
                    get(routes::list_sessions),
                )
                .route(
                    "/api/v1/apps/{app_id}/agents/{agent_id}/sessions/{session_id}",
                    get(routes::get_session),
                ),
        )
    }
}
