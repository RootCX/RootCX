pub mod policy;
mod routes;

use async_trait::async_trait;
use axum::Router;
use axum::routing::{get, patch, post};
use sqlx::PgPool;
use tracing::info;
use uuid::Uuid;

use super::RuntimeExtension;
use crate::RuntimeError;
use crate::routes::SharedRuntime;
use rootcx_types::AppManifest;

pub struct RbacExtension;

async fn exec(pool: &PgPool, sql: &str) -> Result<(), RuntimeError> {
    sqlx::query(sql).execute(pool).await.map_err(RuntimeError::Schema)?;
    Ok(())
}

#[async_trait]
impl RuntimeExtension for RbacExtension {
    fn name(&self) -> &str { "rbac" }

    async fn bootstrap(&self, pool: &PgPool) -> Result<(), RuntimeError> {
        info!("bootstrapping RBAC extension");

        // ── Migration: per-app → global ────────────────────────────────
        // Detect old schema (app_id exists in rbac_roles PK) and migrate.
        let has_old_schema: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM information_schema.columns
                WHERE table_schema = 'rootcx_system' AND table_name = 'rbac_roles' AND column_name = 'app_id'
            )",
        ).fetch_one(pool).await.map_err(RuntimeError::Schema)?;

        if has_old_schema {
            info!("migrating RBAC from per-app to global schema");
            self.migrate_to_global(pool).await?;
        }

        // ── Global schema ──────────────────────────────────────────────
        for ddl in [
            "CREATE TABLE IF NOT EXISTS rootcx_system.rbac_permissions (
                key         TEXT PRIMARY KEY,
                description TEXT NOT NULL DEFAULT '',
                source_app  TEXT
            )",
            "CREATE TABLE IF NOT EXISTS rootcx_system.rbac_roles (
                name        TEXT PRIMARY KEY,
                description TEXT,
                inherits    TEXT[] NOT NULL DEFAULT '{}',
                permissions TEXT[] NOT NULL DEFAULT '{}'
            )",
            "CREATE TABLE IF NOT EXISTS rootcx_system.rbac_assignments (
                user_id     UUID NOT NULL REFERENCES rootcx_system.users(id) ON DELETE CASCADE,
                role        TEXT NOT NULL,
                assigned_at TIMESTAMPTZ NOT NULL DEFAULT now(),
                PRIMARY KEY (user_id, role)
            )",
            "CREATE INDEX IF NOT EXISTS idx_rbac_assignments_user
                ON rootcx_system.rbac_assignments (user_id)",
        ] {
            exec(pool, ddl).await?;
        }

        exec(pool,
            "INSERT INTO rootcx_system.rbac_roles (name, description, permissions)
             VALUES ('admin', 'Instance administrator', ARRAY['*'])
             ON CONFLICT (name) DO NOTHING",
        ).await?;

        // ── Migration: namespace permission keys ───────────────────────
        // Detect old-format keys (tool.X, integration.X.Y, app keys without app: prefix)
        let has_old_keys: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM rootcx_system.rbac_permissions WHERE key LIKE 'tool.%' OR key LIKE 'integration.%')",
        ).fetch_one(pool).await.map_err(RuntimeError::Schema)?;

        if has_old_keys {
            info!("migrating permission keys to namespaced format");
            self.migrate_permission_keys(pool).await?;
        }

        info!("RBAC extension ready");
        Ok(())
    }

    async fn on_app_installed(&self, pool: &PgPool, manifest: &AppManifest, installed_by: Uuid) -> Result<(), RuntimeError> {
        let app_id = &manifest.app_id;

        let (keys, descs): (Vec<String>, Vec<String>) = if let Some(c) = &manifest.permissions {
            c.permissions.iter().map(|p| (format!("app:{app_id}:{}", p.key), p.description.clone())).unzip()
        } else {
            manifest.data_contract.iter()
                .flat_map(|e| ["create", "read", "update", "delete"]
                    .map(|a| (format!("app:{app_id}:{}.{a}", e.entity_name), format!("{a} {}", e.entity_name))))
                .unzip()
        };

        let mut tx = pool.begin().await.map_err(RuntimeError::Schema)?;
        sqlx::query("DELETE FROM rootcx_system.rbac_permissions WHERE source_app = $1")
            .bind(app_id).execute(&mut *tx).await.map_err(RuntimeError::Schema)?;
        sqlx::query(
            "INSERT INTO rootcx_system.rbac_permissions (key, description, source_app)
             SELECT unnest($1::text[]), unnest($2::text[]), $3")
            .bind(&keys).bind(&descs).bind(app_id)
            .execute(&mut *tx).await.map_err(RuntimeError::Schema)?;
        tx.commit().await.map_err(RuntimeError::Schema)?;

        sqlx::query(
            "INSERT INTO rootcx_system.rbac_assignments (user_id, role)
             VALUES ($1, 'admin') ON CONFLICT DO NOTHING",
        ).bind(installed_by).execute(pool).await.map_err(RuntimeError::Schema)?;
        info!(app = %app_id, user = %installed_by, "installer promoted to admin");
        Ok(())
    }

    fn routes(&self) -> Option<Router<SharedRuntime>> {
        Some(Router::new()
            .route("/api/v1/roles", get(routes::list_roles).post(routes::create_role))
            .route("/api/v1/roles/{role_name}", patch(routes::update_role).delete(routes::delete_role))
            .route("/api/v1/roles/assignments", get(routes::list_assignments))
            .route("/api/v1/roles/assign", post(routes::assign_role))
            .route("/api/v1/roles/revoke", post(routes::revoke_role))
            .route("/api/v1/permissions", get(routes::my_permissions))
            .route("/api/v1/permissions/available", get(routes::list_available_permissions))
            .route("/api/v1/permissions/{user_id}", get(routes::user_permissions)))
    }
}

impl RbacExtension {
    /// One-shot migration from per-app (old) to global (new) schema.
    async fn migrate_to_global(&self, pool: &PgPool) -> Result<(), RuntimeError> {
        let mut tx = pool.begin().await.map_err(RuntimeError::Schema)?;

        // 1. Create new tables
        for ddl in [
            "CREATE TABLE IF NOT EXISTS rootcx_system._rbac_permissions_new (
                key TEXT PRIMARY KEY, description TEXT NOT NULL DEFAULT '', source_app TEXT
            )",
            "CREATE TABLE IF NOT EXISTS rootcx_system._rbac_roles_new (
                name TEXT PRIMARY KEY, description TEXT,
                inherits TEXT[] NOT NULL DEFAULT '{}', permissions TEXT[] NOT NULL DEFAULT '{}'
            )",
            "CREATE TABLE IF NOT EXISTS rootcx_system._rbac_assignments_new (
                user_id UUID NOT NULL REFERENCES rootcx_system.users(id) ON DELETE CASCADE,
                role TEXT NOT NULL, assigned_at TIMESTAMPTZ NOT NULL DEFAULT now(),
                PRIMARY KEY (user_id, role)
            )",
        ] {
            sqlx::query(ddl).execute(&mut *tx).await.map_err(RuntimeError::Schema)?;
        }

        // 2. Migrate permissions — namespace with type prefix
        sqlx::query(
            "INSERT INTO rootcx_system._rbac_permissions_new (key, description, source_app)
             SELECT CASE
                 WHEN key LIKE 'tool.%' THEN 'tool:' || substring(key FROM 6)
                 WHEN key LIKE 'integration.%' THEN 'integration:' || replace(substring(key FROM 13), '.', ':')
                 ELSE 'app:' || app_id || ':' || key
             END, description, app_id
             FROM rootcx_system.rbac_permissions
             ON CONFLICT (key) DO NOTHING"
        ).execute(&mut *tx).await.map_err(RuntimeError::Schema)?;

        // 3. Migrate roles — prefix non-core, non-admin names if conflicts
        sqlx::query(
            "INSERT INTO rootcx_system._rbac_roles_new (name, description, inherits, permissions)
             SELECT
                 CASE WHEN app_id = 'core' THEN name
                      ELSE CASE WHEN EXISTS(
                          SELECT 1 FROM rootcx_system.rbac_roles r2
                          WHERE r2.app_id != rootcx_system.rbac_roles.app_id AND r2.name = rootcx_system.rbac_roles.name
                      ) THEN app_id || ':' || name ELSE name END
                 END,
                 description, inherits,
                 CASE WHEN app_id = 'core' THEN permissions
                      ELSE ARRAY(
                          SELECT CASE
                              WHEN p = '*' THEN 'app:' || app_id || ':*'
                              WHEN p LIKE 'tool.%' THEN 'tool:' || substring(p FROM 6)
                              WHEN p LIKE 'integration.%' THEN 'integration:' || replace(substring(p FROM 13), '.', ':')
                              ELSE 'app:' || app_id || ':' || p
                          END
                          FROM unnest(permissions) AS p
                      )
                 END
             FROM rootcx_system.rbac_roles
             ON CONFLICT (name) DO UPDATE SET
                 permissions = ARRAY(
                     SELECT DISTINCT unnest(
                         rootcx_system._rbac_roles_new.permissions || EXCLUDED.permissions
                     )
                 )"
        ).execute(&mut *tx).await.map_err(RuntimeError::Schema)?;

        // 4. Migrate assignments — deduplicate across apps
        sqlx::query(
            "INSERT INTO rootcx_system._rbac_assignments_new (user_id, role, assigned_at)
             SELECT a.user_id,
                    CASE WHEN a.app_id = 'core' THEN a.role
                         ELSE CASE WHEN EXISTS(
                             SELECT 1 FROM rootcx_system.rbac_roles r2
                             WHERE r2.app_id != a.app_id AND r2.name = a.role
                         ) THEN a.app_id || ':' || a.role ELSE a.role END
                    END,
                    a.assigned_at
             FROM rootcx_system.rbac_assignments a
             ON CONFLICT (user_id, role) DO NOTHING"
        ).execute(&mut *tx).await.map_err(RuntimeError::Schema)?;

        // 5. Swap tables
        for sql in [
            "DROP TABLE rootcx_system.rbac_assignments",
            "DROP TABLE rootcx_system.rbac_roles",
            "DROP TABLE rootcx_system.rbac_permissions",
            "ALTER TABLE rootcx_system._rbac_permissions_new RENAME TO rbac_permissions",
            "ALTER TABLE rootcx_system._rbac_roles_new RENAME TO rbac_roles",
            "ALTER TABLE rootcx_system._rbac_assignments_new RENAME TO rbac_assignments",
        ] {
            sqlx::query(sql).execute(&mut *tx).await.map_err(RuntimeError::Schema)?;
        }

        tx.commit().await.map_err(RuntimeError::Schema)?;
        info!("RBAC migration to global schema completed");
        Ok(())
    }

    /// Rename permission keys: tool.X → tool:X, integration.X.Y → integration:X:Y,
    /// {app}:X → app:{app}:X. Also updates role permission arrays.
    async fn migrate_permission_keys(&self, pool: &PgPool) -> Result<(), RuntimeError> {
        let mut tx = pool.begin().await.map_err(RuntimeError::Schema)?;

        for sql in [
            "UPDATE rootcx_system.rbac_permissions SET key = 'tool:' || substring(key FROM 6) WHERE key LIKE 'tool.%'",
            "UPDATE rootcx_system.rbac_permissions SET key = 'integration:' || replace(substring(key FROM 13), '.', ':') WHERE key LIKE 'integration.%'",
            "UPDATE rootcx_system.rbac_permissions SET key = 'app:' || key WHERE source_app IS NOT NULL AND key NOT LIKE 'app:%' AND key NOT LIKE 'tool:%' AND key NOT LIKE 'integration:%'",
        ] {
            sqlx::query(sql).execute(&mut *tx).await.map_err(RuntimeError::Schema)?;
        }

        sqlx::query(
            "UPDATE rootcx_system.rbac_roles SET permissions = ARRAY(
                SELECT CASE
                    WHEN p LIKE 'tool.%' THEN 'tool:' || substring(p FROM 6)
                    WHEN p LIKE 'integration.%' THEN 'integration:' || replace(substring(p FROM 13), '.', ':')
                    WHEN p LIKE '%:*' AND p NOT LIKE 'app:%' AND p NOT LIKE 'tool:%' AND p NOT LIKE 'integration:%'
                        THEN 'app:' || p
                    WHEN p LIKE '%:%' AND p NOT LIKE 'app:%' AND p NOT LIKE 'tool:%' AND p NOT LIKE 'integration:%' AND p != '*'
                        THEN 'app:' || p
                    ELSE p
                END FROM unnest(permissions) AS p
            ) WHERE permissions != '{}'"
        ).execute(&mut *tx).await.map_err(RuntimeError::Schema)?;

        tx.commit().await.map_err(RuntimeError::Schema)?;
        info!("permission keys migrated to namespaced format");
        Ok(())
    }
}
