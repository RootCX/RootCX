pub(crate) mod routes;

use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;
use axum::routing::{get, post};
use sqlx::PgPool;
use tracing::info;
use uuid::Uuid;

use super::RuntimeExtension;
use crate::RuntimeError;
use crate::extensions::rbac::PolicyCache;
use crate::routes::SharedRuntime;
use rootcx_shared_types::AppManifest;

pub fn agent_user_id(app_id: &str) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_OID, format!("agent:{app_id}").as_bytes())
}

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
                app_id      TEXT PRIMARY KEY REFERENCES rootcx_system.apps(id) ON DELETE CASCADE,
                name        TEXT NOT NULL,
                description TEXT,
                model       TEXT,
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
        ] {
            sqlx::query(ddl).execute(pool).await.map_err(RuntimeError::Schema)?;
        }
        info!("agents extension ready");
        Ok(())
    }

    async fn on_app_installed(&self, pool: &PgPool, manifest: &AppManifest) -> Result<(), RuntimeError> {
        let def = match &manifest.agent {
            Some(d) => d,
            None => return Ok(()),
        };
        let app_id = &manifest.app_id;
        let role_name = format!("agent:{app_id}");

        let config = serde_json::json!({
            "systemPrompt": def.system_prompt,
            "graph": def.graph,
            "memory": def.memory,
            "limits": def.limits,
            "access": def.access,
        });
        sqlx::query(
            "INSERT INTO rootcx_system.agents (app_id, name, description, model, config)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (app_id) DO UPDATE SET
                 name = EXCLUDED.name, description = EXCLUDED.description,
                 model = EXCLUDED.model, config = EXCLUDED.config, updated_at = now()"
        )
        .bind(app_id)
        .bind(&def.name)
        .bind(def.description.as_deref())
        .bind(def.model.as_deref())
        .bind(&config)
        .execute(pool)
        .await
        .map_err(RuntimeError::Schema)?;

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

        let agent_uid = agent_user_id(app_id);
        sqlx::query(
            "INSERT INTO rootcx_system.users (id, username, is_system)
             VALUES ($1, $2, true)
             ON CONFLICT (id) DO NOTHING"
        )
        .bind(agent_uid)
        .bind(&role_name)
        .execute(pool)
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
        .execute(pool)
        .await
        .map_err(RuntimeError::Schema)?;

        sqlx::query("DELETE FROM rootcx_system.rbac_policies WHERE app_id = $1 AND role = $2")
            .bind(app_id)
            .bind(&role_name)
            .execute(pool)
            .await
            .map_err(RuntimeError::Schema)?;

        let data_entries: Vec<_> = def.access.iter()
            .filter(|e| !e.entity.starts_with("tool:"))
            .collect();
        if !data_entries.is_empty() {
            let mut sql = String::from(
                "INSERT INTO rootcx_system.rbac_policies (app_id, role, entity, actions, ownership) VALUES "
            );
            for (i, _) in data_entries.iter().enumerate() {
                let off = i * 2 + 3;
                if i > 0 { sql.push(','); }
                sql.push_str(&format!("($1,$2,${},${},false)", off, off + 1));
            }
            let mut q = sqlx::query(&sql).bind(app_id).bind(&role_name);
            for entry in &data_entries {
                let actions: Vec<&str> = entry.actions.iter().map(String::as_str).collect();
                q = q.bind(&entry.entity).bind(actions);
            }
            q.execute(pool).await.map_err(RuntimeError::Schema)?;
        }

        self.rbac_cache.invalidate(app_id);
        info!(app = %app_id, "agent registered");
        Ok(())
    }

    fn routes(&self) -> Option<Router<SharedRuntime>> {
        Some(
            Router::new()
                .route("/api/v1/apps/{app_id}/agent", get(routes::get_agent))
                .route("/api/v1/apps/{app_id}/agent/invoke", post(routes::invoke_agent))
                .route("/api/v1/apps/{app_id}/agent/sessions", get(routes::list_sessions))
                .route("/api/v1/apps/{app_id}/agent/sessions/{session_id}", get(routes::get_session)),
        )
    }
}
