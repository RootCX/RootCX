mod bootstrap;
use async_trait::async_trait;
use axum::Router;
use axum::routing::{get, patch, post};
use sqlx::PgPool;
use tracing::info;
use uuid::Uuid;

use super::RuntimeExtension;
use crate::RuntimeError;
use crate::governance::authority::routes;
use crate::routes::SharedRuntime;
use rootcx_types::AppManifest;

pub struct RbacExtension;

pub(super) async fn exec(pool: &PgPool, sql: &str) -> Result<(), RuntimeError> {
    sqlx::query(sql).execute(pool).await.map_err(RuntimeError::Schema)?;
    Ok(())
}

/// True until the first non-system user is assigned any role. The seeded
/// system user holds admin for RLS, so it is excluded. Drives the one-time
/// "first install promotes admin" bootstrap and the install/uninstall gate.
pub async fn is_first_boot(pool: &PgPool) -> Result<bool, crate::api_error::ApiError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT NOT EXISTS(SELECT 1 FROM rootcx_system.rbac_assignments a \
         JOIN rootcx_system.users u ON u.id = a.user_id WHERE NOT u.is_system)",
    ).fetch_one(pool).await?)
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

        // Deny-by-default base role for federated/invited users. Holds no
        // permissions; real access comes from explicitly-granted roles. This is
        // the safe fallback `default_role` for the OIDC provider — never `admin`.
        // Named `base` (not `member`) to avoid collision with the website's
        // platform `member` role, which is a separate concept in a separate DB.
        exec(pool,
            "INSERT INTO rootcx_system.rbac_roles (name, description, permissions)
             VALUES ('base', 'Base role — no permissions by default', ARRAY[]::text[])
             ON CONFLICT (name) DO NOTHING",
        ).await?;

        exec(pool,
            "INSERT INTO rootcx_system.rbac_permissions (key, description) \
             VALUES ('platform:apps.create', 'Create and own apps (self-service)') \
             ON CONFLICT (key) DO NOTHING",
        ).await?;

        self.bootstrap_governance(pool).await?;

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

        // Per-entity CRUD + cron keys are what the table RLS policies require
        // (apply_table_rls gates on `app:{schema}:{entity}.{action}`), so they
        // must exist for EVERY app. Mint them unconditionally, then append any
        // custom keys the manifest declares. (A custom block used to REPLACE
        // these, leaving custom-permission apps with ungrantable, invisible
        // entity keys and deny-all data for every non-admin.)
        let (mut keys, mut descs): (Vec<String>, Vec<String>) = manifest.data_contract.iter()
            .flat_map(|e| ["create", "read", "update", "delete"]
                .map(|a| (format!("app:{app_id}:{}.{a}", e.entity_name), format!("{a} {}", e.entity_name))))
            .chain(["read", "write", "trigger"].into_iter()
                .map(|a| (format!("app:{app_id}:cron.{a}"), format!("{a} crons"))))
            .unzip();

        if let Some(c) = &manifest.permissions {
            for p in &c.permissions {
                keys.push(format!("app:{app_id}:{}", p.key));
                descs.push(p.description.clone());
            }
        }

        // Always generate the invoke permission so it is grantable per role
        keys.push(format!("app:{app_id}:invoke"));
        descs.push("invoke the app's agent".into());

        let mut tx = pool.begin().await.map_err(RuntimeError::Schema)?;
        sqlx::query("DELETE FROM rootcx_system.rbac_permissions WHERE source_app = $1")
            .bind(app_id).execute(&mut *tx).await.map_err(RuntimeError::Schema)?;
        sqlx::query(
            "INSERT INTO rootcx_system.rbac_permissions (key, description, source_app)
             SELECT unnest($1::text[]), unnest($2::text[]), $3
             ON CONFLICT (key) DO NOTHING")
            .bind(&keys).bind(&descs).bind(app_id)
            .execute(&mut *tx).await.map_err(RuntimeError::Schema)?;
        tx.commit().await.map_err(RuntimeError::Schema)?;

        // First-boot only: promote the very first installer to platform admin.
        if is_first_boot(pool).await.unwrap_or(false) {
            sqlx::query(
                "INSERT INTO rootcx_system.rbac_assignments (user_id, role)
                 VALUES ($1, 'admin') ON CONFLICT DO NOTHING",
            ).bind(installed_by).execute(pool).await.map_err(RuntimeError::Schema)?;
            info!(app = %app_id, user = %installed_by, "first-boot: installer promoted to admin");
        }

        // Auto-assign the installer as app admin: a role carrying `app:{id}:*`
        // gives full control over this app (data, crons, hooks, invoke, deploy)
        // without granting platform-level authority.
        let owner_role = format!("app:{app_id}:admin");
        let owner_perms = vec![format!("app:{app_id}:*")];
        sqlx::query(
            "INSERT INTO rootcx_system.rbac_roles (name, description, permissions) \
             VALUES ($1, $2, $3) ON CONFLICT (name) DO UPDATE SET permissions = EXCLUDED.permissions",
        )
        .bind(&owner_role)
        .bind(format!("{app_id} app administrator"))
        .bind(&owner_perms)
        .execute(pool).await.map_err(RuntimeError::Schema)?;
        sqlx::query(
            "INSERT INTO rootcx_system.rbac_assignments (user_id, role) \
             VALUES ($1, $2) ON CONFLICT DO NOTHING",
        )
        .bind(installed_by)
        .bind(&owner_role)
        .execute(pool).await.map_err(RuntimeError::Schema)?;
        info!(app = %app_id, user = %installed_by, "app admin role assigned");

        Ok(())
    }

    async fn on_table_created(
        &self,
        pool: &PgPool,
        _manifest: &AppManifest,
        schema: &str,
        table: &str,
    ) -> Result<(), RuntimeError> {
        apply_table_rls(pool, schema, table).await
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

/// Enable + FORCE RLS on an app table, grant the restricted executor CRUD on
/// it, and (re)create the four permission-gated policies. Idempotent: safe to
/// call on every install and on the retroactive boot pass. `schema`/`table`
/// are validated snake_case identifiers (see `manifest::validate_manifest`).
pub(crate) async fn apply_table_rls(pool: &PgPool, schema: &str, table: &str) -> Result<(), RuntimeError> {
    use crate::manifest::{quote_ident, quote_literal};
    let qt = format!("{}.{}", quote_ident(schema), quote_ident(table));

    exec(pool, &format!("GRANT USAGE ON SCHEMA {} TO rootcx_app_executor", quote_ident(schema))).await?;
    exec(pool, &format!("GRANT SELECT, INSERT, UPDATE, DELETE ON {qt} TO rootcx_app_executor")).await?;
    exec(pool, &format!(
        "ALTER DEFAULT PRIVILEGES IN SCHEMA {} GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO rootcx_app_executor",
        quote_ident(schema),
    )).await?;
    exec(pool, &format!("ALTER TABLE {qt} ENABLE ROW LEVEL SECURITY")).await?;
    exec(pool, &format!("ALTER TABLE {qt} FORCE ROW LEVEL SECURITY")).await?;

    // The `(SELECT ...)` wrapper makes the planner evaluate check_access once
    // per query (constant args) instead of once per row — mandatory for perf.
    for (policy, kind, action) in [
        ("rootcx_rls_select", "FOR SELECT USING", "read"),
        ("rootcx_rls_insert", "FOR INSERT WITH CHECK", "create"),
        ("rootcx_rls_delete", "FOR DELETE USING", "delete"),
    ] {
        let req = quote_literal(&format!("app:{schema}:{table}.{action}"));
        exec(pool, &format!("DROP POLICY IF EXISTS {policy} ON {qt}")).await?;
        exec(pool, &format!(
            "CREATE POLICY {policy} ON {qt} {kind} ((SELECT rootcx_system.check_access({req})))",
        )).await?;
    }
    // UPDATE needs both USING (visible rows) and WITH CHECK (new rows).
    let upd = quote_literal(&format!("app:{schema}:{table}.update"));
    exec(pool, &format!("DROP POLICY IF EXISTS rootcx_rls_update ON {qt}")).await?;
    exec(pool, &format!(
        "CREATE POLICY rootcx_rls_update ON {qt} FOR UPDATE \
         USING ((SELECT rootcx_system.check_access({upd}))) \
         WITH CHECK ((SELECT rootcx_system.check_access({upd})))",
    )).await?;

    Ok(())
}
