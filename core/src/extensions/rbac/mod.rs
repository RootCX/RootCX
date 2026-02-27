pub mod policy;
mod routes;

use async_trait::async_trait;
use axum::Router;
use axum::routing::{delete, get, patch, post};
use sqlx::PgPool;
use tracing::info;
use uuid::Uuid;

use super::RuntimeExtension;
use crate::RuntimeError;
use crate::routes::SharedRuntime;
use rootcx_shared_types::AppManifest;

pub struct RbacExtension;

async fn exec(pool: &PgPool, sql: &str) -> Result<(), RuntimeError> {
    sqlx::query(sql).execute(pool).await.map_err(RuntimeError::Schema)?;
    Ok(())
}

#[async_trait]
impl RuntimeExtension for RbacExtension {
    fn name(&self) -> &str {
        "rbac"
    }

    async fn bootstrap(&self, pool: &PgPool) -> Result<(), RuntimeError> {
        info!("bootstrapping RBAC extension");
        for ddl in [
            "CREATE TABLE IF NOT EXISTS rootcx_system.rbac_permissions (
                app_id TEXT NOT NULL REFERENCES rootcx_system.apps(id) ON DELETE CASCADE,
                key    TEXT NOT NULL,
                description TEXT NOT NULL DEFAULT '',
                PRIMARY KEY (app_id, key)
            )",
            "CREATE TABLE IF NOT EXISTS rootcx_system.rbac_roles (
                app_id      TEXT NOT NULL REFERENCES rootcx_system.apps(id) ON DELETE CASCADE,
                name        TEXT NOT NULL,
                description TEXT,
                inherits    TEXT[] NOT NULL DEFAULT '{}',
                permissions TEXT[] NOT NULL DEFAULT '{}',
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
            // Add permissions column to existing rbac_roles tables (idempotent migration)
            "ALTER TABLE rootcx_system.rbac_roles ADD COLUMN IF NOT EXISTS permissions TEXT[] NOT NULL DEFAULT '{}'",
            // Drop legacy rbac_policies table if it exists
            "DROP TABLE IF EXISTS rootcx_system.rbac_policies",
        ] {
            exec(pool, ddl).await?;
        }
        info!("RBAC extension ready");
        Ok(())
    }

    async fn on_app_installed(&self, pool: &PgPool, manifest: &AppManifest, installed_by: Uuid, tool_names: &[String]) -> Result<(), RuntimeError> {
        let app_id = &manifest.app_id;

        let mut perms: Vec<(String, String)> = Vec::new();
        if let Some(contract) = &manifest.permissions {
            perms.extend(contract.permissions.iter().map(|p| (p.key.clone(), p.description.clone())));
        }
        for entity in &manifest.data_contract {
            for action in ["create", "read", "update", "delete"] {
                perms.push((
                    format!("{}.{action}", entity.entity_name),
                    format!("{action} {}", entity.entity_name),
                ));
            }
        }
        for name in tool_names {
            perms.push((format!("tool.{name}"), format!("use {name} tool")));
        }

        // Single atomic sync: delete stale + insert current
        let mut tx = pool.begin().await.map_err(RuntimeError::Schema)?;
        sqlx::query("DELETE FROM rootcx_system.rbac_permissions WHERE app_id = $1")
            .bind(app_id).execute(&mut *tx).await.map_err(RuntimeError::Schema)?;
        for (key, desc) in &perms {
            sqlx::query("INSERT INTO rootcx_system.rbac_permissions (app_id, key, description) VALUES ($1, $2, $3)")
                .bind(app_id).bind(key).bind(desc)
                .execute(&mut *tx).await.map_err(RuntimeError::Schema)?;
        }
        tx.commit().await.map_err(RuntimeError::Schema)?;

        // Create built-in admin role with wildcard permission if not exists
        sqlx::query(
            "INSERT INTO rootcx_system.rbac_roles (app_id, name, description, permissions)
             VALUES ($1, 'admin', 'Built-in administrator role', ARRAY['*'])
             ON CONFLICT (app_id, name) DO UPDATE SET permissions = ARRAY['*']",
        )
        .bind(app_id)
        .execute(pool)
        .await
        .map_err(RuntimeError::Schema)?;

        // Auto-assign installer to admin role
        sqlx::query(
            "INSERT INTO rootcx_system.rbac_assignments (user_id, app_id, role) \
             VALUES ($1, $2, 'admin') ON CONFLICT DO NOTHING",
        )
        .bind(installed_by)
        .bind(app_id)
        .execute(pool)
        .await
        .map_err(RuntimeError::Schema)?;
        info!(app = %app_id, user = %installed_by, "installer promoted to admin");

        Ok(())
    }

    fn routes(&self) -> Option<Router<SharedRuntime>> {
        Some(
            Router::new()
                .route("/api/v1/apps/{app_id}/roles", get(routes::list_roles))
                .route("/api/v1/apps/{app_id}/roles", post(routes::create_role))
                .route("/api/v1/apps/{app_id}/roles/{role_name}", patch(routes::update_role))
                .route("/api/v1/apps/{app_id}/roles/{role_name}", delete(routes::delete_role))
                .route("/api/v1/apps/{app_id}/roles/assignments", get(routes::list_assignments))
                .route("/api/v1/apps/{app_id}/roles/assign", post(routes::assign_role))
                .route("/api/v1/apps/{app_id}/roles/revoke", post(routes::revoke_role))
                .route("/api/v1/apps/{app_id}/permissions", get(routes::my_permissions))
                .route("/api/v1/apps/{app_id}/permissions/available", get(routes::list_available_permissions))
                .route("/api/v1/apps/{app_id}/permissions/{user_id}", get(routes::user_permissions)),
        )
    }
}
