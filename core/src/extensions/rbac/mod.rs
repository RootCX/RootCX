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

/// The per-entity actions minted as permission keys, both unscoped and as `.own`
/// twins. Shared by the two minting sites so neither can gain an action the other
/// lacks. It must also match the actions `RLS_POLICIES` gates on — a key with no
/// policy is an unenforceable grant, a policy with no key is unreachable — which
/// `minted_actions_match_the_policies` checks, since the two lists are built for
/// different purposes and cannot simply be one.
pub(crate) const ENTITY_ACTIONS: [&str; 4] = ["create", "read", "update", "delete"];

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

        for (key, description) in [
            ("admin:cross_app.grants.manage", "Create, inspect and revoke cross-app collection grants"),
            ("admin:cross_app.grants.approve", "Approve cross-app collection grants"),
        ] {
            exec(pool, &format!(
                "INSERT INTO rootcx_system.rbac_permissions (key, description) \
                 VALUES ('{key}', '{description}') ON CONFLICT (key) DO NOTHING"
            )).await?;
        }

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

        self.warn_on_reserved_scope_keys(pool).await?;

        info!("RBAC extension ready");
        Ok(())
    }

    async fn on_app_installed(&self, pool: &PgPool, manifest: &AppManifest, installed_by: Uuid) -> Result<(), RuntimeError> {
        crate::governance::row_access::install(pool, manifest).await?;
        let app_id = &manifest.app_id;

        // Per-entity CRUD + cron keys are what the table RLS policies require
        // (apply_table_rls gates on `app:{schema}:{entity}.{action}`), so they
        // must exist for EVERY app. Mint them unconditionally, then append any
        // custom keys the manifest declares. (A custom block used to REPLACE
        // these, leaving custom-permission apps with ungrantable, invisible
        // entity keys and deny-all data for every non-admin.)
        let (mut keys, mut descs): (Vec<String>, Vec<String>) = manifest.data_contract.iter()
            .flat_map(|e| ENTITY_ACTIONS
                .map(|a| (format!("app:{app_id}:{}.{a}", e.entity_name), format!("{a} {}", e.entity_name))))
            .chain(["read", "write", "trigger"].into_iter()
                .map(|a| (format!("app:{app_id}:cron.{a}"), format!("{a} crons"))))
            .chain(["read", "write"].into_iter()
                .map(|a| (format!("app:{app_id}:hook.{a}"), format!("{a} entity hooks"))))
            .chain([("read", "read app files"), ("write", "write app files")]
                .map(|(a, description)| (format!("app:{app_id}:storage.{a}"), description.into())))
            .unzip();

        // Row-scoped twins, only where a row can be owned. Described distinctly
        // from the unscoped key: the role picker lists them adjacently, so two
        // rows both reading "read contacts" would be a coin toss for the operator.
        // `OWN_SCOPE` rather than a literal: the suffix is reserved at the
        // declaration door from the same constant, so the key an app cannot declare
        // and the key the core mints can never drift apart.
        for entity in manifest.data_contract.iter().filter(|e| crate::manifest::owner_field(e).is_some()) {
            for action in ENTITY_ACTIONS {
                keys.push(format!("app:{app_id}:{}.{action}.{}", entity.entity_name, crate::manifest::OWN_SCOPE));
                descs.push(format!("{action} only their own {}", entity.entity_name));
            }
        }
        for entity in crate::governance::row_access::shared_entities(manifest) {
            keys.push(format!("app:{app_id}:{}.read.shared", entity.entity_name));
            descs.push(format!("read {} shared through active assignments", entity.entity_name));
        }

        if let Some(c) = &manifest.permissions {
            for p in &c.permissions {
                keys.push(format!("app:{app_id}:{}", p.key));
                descs.push(p.description.clone());
            }
        }

        // Always generate the invoke permission so it is grantable per role
        keys.push(format!("app:{app_id}:invoke"));
        descs.push("invoke the app's agent".into());

        // Provider-local approval is separate from platform-wide grant
        // management.  A provider owner may approve access to its own data
        // without gaining the ability to create or revoke unrelated grants.
        keys.push(format!("app:{app_id}:cross_app.approve"));
        descs.push("approve cross-app collection grants targeting this app".into());

        // Reaching another principal's trigger is a separate, elevated grant: a
        // hook or cron receives whatever its owner can reach, so managing your
        // own does not imply managing theirs. (`cron.manage_others` was already
        // enforced but never minted, so it was invisible in the role picker.)
        for kind in ["cron", "hook"] {
            keys.push(format!("app:{app_id}:{kind}.manage_others"));
            descs.push(format!("read and delete {kind}s owned by other users"));
        }

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

// Compatibility for internal callers while policy ownership lives in governance.
pub(crate) use crate::governance::row_access::{govern_schema_tables, prune_owner_resolvers};
