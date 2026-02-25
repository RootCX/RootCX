pub(crate) mod grant;
pub mod policy;
mod routes;

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;
use axum::routing::{get, post};
use sqlx::PgPool;
use tracing::info;

pub use grant::AccessGrant;
pub use policy::PolicyCache;

use super::RuntimeExtension;
use crate::RuntimeError;
use crate::manifest::quote_ident;
use crate::routes::SharedRuntime;
use rootcx_shared_types::AppManifest;

pub struct RbacExtension {
    cache: Arc<PolicyCache>,
}

impl RbacExtension {
    pub fn with_cache(cache: Arc<PolicyCache>) -> Self {
        Self { cache }
    }
}

async fn exec(pool: &PgPool, sql: &str) -> Result<(), RuntimeError> {
    sqlx::query(sql).execute(pool).await.map_err(RuntimeError::Schema)?;
    Ok(())
}

fn validation_err(msg: String) -> RuntimeError {
    RuntimeError::Schema(sqlx::Error::Protocol(msg))
}

#[async_trait]
impl RuntimeExtension for RbacExtension {
    fn name(&self) -> &str {
        "rbac"
    }

    async fn bootstrap(&self, pool: &PgPool) -> Result<(), RuntimeError> {
        info!("bootstrapping RBAC extension");
        for ddl in [
            "CREATE TABLE IF NOT EXISTS rootcx_system.rbac_roles (
                app_id TEXT NOT NULL REFERENCES rootcx_system.apps(id) ON DELETE CASCADE,
                name TEXT NOT NULL, description TEXT,
                inherits TEXT[] NOT NULL DEFAULT '{}',
                PRIMARY KEY (app_id, name)
            )",
            "CREATE TABLE IF NOT EXISTS rootcx_system.rbac_assignments (
                user_id UUID NOT NULL REFERENCES rootcx_system.users(id) ON DELETE CASCADE,
                app_id TEXT NOT NULL REFERENCES rootcx_system.apps(id) ON DELETE CASCADE,
                role TEXT NOT NULL, assigned_at TIMESTAMPTZ NOT NULL DEFAULT now(),
                PRIMARY KEY (user_id, app_id, role)
            )",
            "CREATE INDEX IF NOT EXISTS idx_rbac_assignments_user_app
                ON rootcx_system.rbac_assignments (user_id, app_id)",
            "CREATE TABLE IF NOT EXISTS rootcx_system.rbac_policies (
                app_id TEXT NOT NULL REFERENCES rootcx_system.apps(id) ON DELETE CASCADE,
                role TEXT NOT NULL, entity TEXT NOT NULL,
                actions TEXT[] NOT NULL, ownership BOOLEAN NOT NULL DEFAULT false,
                PRIMARY KEY (app_id, role, entity)
            )",
        ] {
            exec(pool, ddl).await?;
        }
        info!("RBAC extension ready");
        Ok(())
    }

    async fn on_table_created(
        &self,
        pool: &PgPool,
        manifest: &AppManifest,
        schema: &str,
        table: &str,
    ) -> Result<(), RuntimeError> {
        let needs_owner = manifest
            .permissions
            .as_ref()
            .is_some_and(|p| p.policies.iter().any(|r| r.ownership && (r.entity == table || r.entity == "*")));
        if !needs_owner {
            return Ok(());
        }

        let fq = format!("{}.{}", quote_ident(schema), quote_ident(table));
        exec(
            pool,
            &format!("ALTER TABLE {fq} ADD COLUMN IF NOT EXISTS owner_id UUID REFERENCES rootcx_system.users(id)"),
        )
        .await?;
        exec(
            pool,
            &format!(
                "CREATE INDEX IF NOT EXISTS {} ON {fq} (owner_id)",
                quote_ident(&format!("idx_{schema}_{table}_owner"))
            ),
        )
        .await?;
        info!(table = %fq, "added owner_id column");
        Ok(())
    }

    async fn on_app_installed(&self, pool: &PgPool, manifest: &AppManifest) -> Result<(), RuntimeError> {
        let contract = match &manifest.permissions {
            Some(c) => c,
            None => return Ok(()),
        };
        let app_id = &manifest.app_id;

        let role_map: HashMap<String, Vec<String>> =
            contract.roles.iter().map(|(k, v)| (k.clone(), v.inherits.clone())).collect();
        if let Some(cycle) = policy::detect_cycle(&role_map) {
            return Err(validation_err(format!("role hierarchy cycle involving '{cycle}'")));
        }
        if let Some(ref d) = contract.default_role
            && !contract.roles.contains_key(d) {
                return Err(validation_err(format!("defaultRole '{d}' not defined")));
            }
        for p in &contract.policies {
            if !contract.roles.contains_key(&p.role) {
                return Err(validation_err(format!("policy references undefined role '{}'", p.role)));
            }
        }

        sqlx::query("DELETE FROM rootcx_system.rbac_policies WHERE app_id = $1")
            .bind(app_id)
            .execute(pool)
            .await
            .map_err(RuntimeError::Schema)?;
        sqlx::query("DELETE FROM rootcx_system.rbac_roles WHERE app_id = $1")
            .bind(app_id)
            .execute(pool)
            .await
            .map_err(RuntimeError::Schema)?;

        for (name, def) in &contract.roles {
            let inherits: Vec<&str> = def.inherits.iter().map(|s| s.as_str()).collect();
            sqlx::query(
                "INSERT INTO rootcx_system.rbac_roles (app_id, name, description, inherits) VALUES ($1, $2, $3, $4)",
            )
            .bind(app_id)
            .bind(name)
            .bind(def.description.as_deref())
            .bind(&inherits)
            .execute(pool)
            .await
            .map_err(RuntimeError::Schema)?;
        }
        for p in &contract.policies {
            let actions: Vec<&str> = p.actions.iter().map(|s| s.as_str()).collect();
            sqlx::query("INSERT INTO rootcx_system.rbac_policies (app_id, role, entity, actions, ownership) VALUES ($1, $2, $3, $4, $5)")
                .bind(app_id).bind(&p.role).bind(&p.entity).bind(&actions).bind(p.ownership)
                .execute(pool).await.map_err(RuntimeError::Schema)?;
        }

        self.cache.invalidate(app_id);
        self.cache.populate(app_id, contract);
        info!(app = %app_id, roles = contract.roles.len(), policies = contract.policies.len(), "RBAC policies synced");
        Ok(())
    }

    fn routes(&self) -> Option<Router<SharedRuntime>> {
        Some(
            Router::new()
                .route("/api/v1/apps/{app_id}/policies", get(routes::list_policies))
                .route("/api/v1/apps/{app_id}/roles", get(routes::list_roles))
                .route("/api/v1/apps/{app_id}/roles/assignments", get(routes::list_assignments))
                .route("/api/v1/apps/{app_id}/roles/assign", post(routes::assign_role))
                .route("/api/v1/apps/{app_id}/roles/revoke", post(routes::revoke_role))
                .route("/api/v1/apps/{app_id}/permissions", get(routes::my_permissions))
                .route("/api/v1/apps/{app_id}/permissions/{user_id}", get(routes::user_permissions))
                .layer(axum::Extension(Arc::clone(&self.cache))),
        )
    }
}
