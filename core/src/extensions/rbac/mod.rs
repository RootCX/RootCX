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

        // First-boot only: promote the very first installer to admin. After an
        // admin exists, installs no longer escalate (the HTTP gate also blocks
        // non-admins at the door — this is defense-in-depth).
        if is_first_boot(pool).await.unwrap_or(false) {
            sqlx::query(
                "INSERT INTO rootcx_system.rbac_assignments (user_id, role)
                 VALUES ($1, 'admin') ON CONFLICT DO NOTHING",
            ).bind(installed_by).execute(pool).await.map_err(RuntimeError::Schema)?;
            info!(app = %app_id, user = %installed_by, "first-boot: installer promoted to admin");
        }
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

    /// Bootstrap the governance DB layer: the restricted `rootcx_app_executor`
    /// role, the plpgsql RBAC functions (single source of truth for RLS and
    /// Rust), the system-schema lockdowns, and a retroactive RLS pass over
    /// pre-existing app tables. Idempotent.
    async fn bootstrap_governance(&self, pool: &PgPool) -> Result<(), RuntimeError> {
        info!("bootstrapping governance (role + plpgsql RBAC + RLS)");

        // Every app table gets FORCE ROW LEVEL SECURITY, which filters even the
        // table owner. Core operations (schema sync, collection_op onStart
        // bypass, retroactive migration) run on this pool connection and rely on
        // it bypassing RLS. Assert that up front — a misconfigured non-superuser
        // role without BYPASSRLS would otherwise silently lose rows / writes.
        let pool_bypasses_rls: bool = sqlx::query_scalar(
            "SELECT rolsuper OR rolbypassrls FROM pg_roles WHERE rolname = current_user",
        ).fetch_one(pool).await.map_err(RuntimeError::Schema)?;
        if !pool_bypasses_rls {
            return Err(RuntimeError::Schema(sqlx::Error::Protocol(
                "the core database role must be a SUPERUSER or have the BYPASSRLS \
                 attribute; otherwise FORCE ROW LEVEL SECURITY filters core operations"
                    .into(),
            )));
        }

        // Note: the system user (...0001) is intentionally NOT seeded with an
        // admin role. Internal operations run on the superuser pool (BYPASSRLS),
        // never through the executor role, so no system identity needs to pass
        // RLS. Seeding it would also defeat the "first registered user becomes
        // admin" bootstrap (the register guard checks for any existing admin).

        // Restricted role used via `SET LOCAL ROLE` for every app query. No
        // login, no RLS bypass — the antithesis of the pool's superuser role.
        exec(pool,
            "DO $$ BEGIN
                IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'rootcx_app_executor') THEN
                    CREATE ROLE rootcx_app_executor NOLOGIN NOBYPASSRLS;
                END IF;
            END $$",
        ).await?;

        // The executor must call the SECURITY DEFINER RBAC functions (USAGE on
        // the schema) but must NOT read system tables (no table grants).
        exec(pool, "REVOKE ALL ON SCHEMA rootcx_system FROM PUBLIC").await?;
        exec(pool, "GRANT USAGE ON SCHEMA rootcx_system TO rootcx_app_executor").await?;

        // The app must never rewrite its identity GUCs from inside its SQL.
        exec(pool, "REVOKE EXECUTE ON FUNCTION pg_catalog.set_config(text, text, boolean) FROM PUBLIC").await?;
        exec(pool, "REVOKE EXECUTE ON FUNCTION pg_catalog.set_config(text, text, boolean) FROM rootcx_app_executor").await?;

        // pgmq/cron carry cross-app job payloads and schedules. Lock them down
        // (guarded — the extensions may not be present in every deployment).
        for schema in ["pgmq", "cron"] {
            exec(pool, &format!(
                "DO $$ BEGIN
                    IF EXISTS (SELECT 1 FROM information_schema.schemata WHERE schema_name = '{schema}') THEN
                        EXECUTE 'REVOKE ALL ON SCHEMA {schema} FROM PUBLIC';
                        EXECUTE 'REVOKE ALL ON ALL TABLES IN SCHEMA {schema} FROM PUBLIC';
                    END IF;
                END $$",
            )).await?;
        }

        // ── plpgsql RBAC: the single implementation shared by RLS and Rust ──
        // Each function is SECURITY DEFINER with a frozen search_path to block
        // hijack via a conflicting object in a higher-priority schema.
        exec(pool,
            "CREATE OR REPLACE FUNCTION rootcx_system.expand_roles(assigned TEXT[])
             RETURNS TEXT[] AS $$
             DECLARE
                 expanded TEXT[] := '{}';
                 stack TEXT[] := COALESCE(assigned, '{}');
                 cur TEXT; parents TEXT[]; p TEXT; depth INT := 0;
             BEGIN
                 WHILE COALESCE(array_length(stack, 1), 0) > 0 AND depth < 64 LOOP
                     cur := stack[array_upper(stack, 1)];
                     stack := stack[1:array_upper(stack, 1) - 1];
                     depth := depth + 1;
                     IF NOT (cur = ANY(expanded)) THEN
                         expanded := array_append(expanded, cur);
                         SELECT inherits INTO parents FROM rootcx_system.rbac_roles WHERE name = cur;
                         IF parents IS NOT NULL THEN
                             FOREACH p IN ARRAY parents LOOP
                                 IF NOT (p = ANY(expanded)) THEN stack := array_append(stack, p); END IF;
                             END LOOP;
                         END IF;
                     END IF;
                 END LOOP;
                 RETURN expanded;
             END;
             $$ LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path = pg_catalog, rootcx_system",
        ).await?;

        exec(pool,
            "CREATE OR REPLACE FUNCTION rootcx_system.resolve_permissions(p_user_id UUID)
             RETURNS TEXT[] AS $$
             DECLARE assigned TEXT[]; all_roles TEXT[]; perms TEXT[];
             BEGIN
                 SELECT array_agg(role) INTO assigned
                     FROM rootcx_system.rbac_assignments WHERE user_id = p_user_id;
                 IF assigned IS NULL OR array_length(assigned, 1) IS NULL THEN RETURN '{}'; END IF;
                 all_roles := rootcx_system.expand_roles(assigned);
                 SELECT array_agg(DISTINCT perm) INTO perms
                     FROM rootcx_system.rbac_roles r, unnest(r.permissions) AS perm
                     WHERE r.name = ANY(all_roles);
                 RETURN COALESCE(perms, '{}');
             END;
             $$ LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path = pg_catalog, rootcx_system",
        ).await?;

        exec(pool,
            "CREATE OR REPLACE FUNCTION rootcx_system.match_permission(p_perms TEXT[], p_required TEXT)
             RETURNS BOOLEAN AS $$
             DECLARE p TEXT; prefix TEXT;
             BEGIN
                 IF p_perms IS NULL THEN RETURN FALSE; END IF;
                 FOREACH p IN ARRAY p_perms LOOP
                     IF p = '*' THEN RETURN TRUE; END IF;
                     IF p = p_required THEN RETURN TRUE; END IF;
                     IF right(p, 2) = ':*' THEN
                         prefix := left(p, length(p) - 2);
                         IF left(p_required, length(prefix)) = prefix
                            AND substr(p_required, length(prefix) + 1, 1) = ':' THEN
                             RETURN TRUE;
                         END IF;
                     END IF;
                 END LOOP;
                 RETURN FALSE;
             END;
             $$ LANGUAGE plpgsql IMMUTABLE SECURITY DEFINER SET search_path = pg_catalog, rootcx_system",
        ).await?;

        exec(pool,
            "CREATE OR REPLACE FUNCTION rootcx_system.has_permission(p_user_id UUID, p_required TEXT)
             RETURNS BOOLEAN AS $$
             BEGIN
                 IF p_user_id IS NULL THEN RETURN FALSE; END IF;
                 RETURN rootcx_system.match_permission(
                     rootcx_system.resolve_permissions(p_user_id), p_required);
             END;
             $$ LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path = pg_catalog, rootcx_system",
        ).await?;

        // The function RLS policies call. Reads the 3 identity GUCs posed by
        // the core. is_delegated='1' → check the pre-computed intersection
        // (empty intersection = deny all); else resolve the user from the DB.
        exec(pool,
            "CREATE OR REPLACE FUNCTION rootcx_system.check_access(p_required TEXT)
             RETURNS BOOLEAN AS $$
             DECLARE v_user_id UUID; v_delegated TEXT; v_perms TEXT;
             BEGIN
                 v_user_id := nullif(current_setting('rootcx.user_id', true), '')::uuid;
                 IF v_user_id IS NULL THEN RETURN FALSE; END IF;
                 v_delegated := current_setting('rootcx.is_delegated', true);
                 IF v_delegated = '1' THEN
                     v_perms := current_setting('rootcx.effective_perms', true);
                     IF v_perms IS NULL OR v_perms = '' THEN RETURN FALSE; END IF;
                     RETURN rootcx_system.match_permission(string_to_array(v_perms, ','), p_required);
                 END IF;
                 RETURN rootcx_system.has_permission(v_user_id, p_required);
             END;
             $$ LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path = pg_catalog, rootcx_system",
        ).await?;

        // Lock down EXECUTE on the RBAC helpers. Only check_access is invoked
        // by the RLS policies (so the executor must keep it); the helpers it
        // calls run as the SECURITY DEFINER owner, so the executor never needs
        // direct EXECUTE on them. New functions default to EXECUTE by PUBLIC, so
        // without this an app could call resolve_permissions/has_permission with
        // an arbitrary user_id via ctx.sql and enumerate the whole RBAC graph
        // (violating Layer 2: "cannot read rootcx_system").
        for sig in [
            "rootcx_system.expand_roles(text[])",
            "rootcx_system.resolve_permissions(uuid)",
            "rootcx_system.match_permission(text[], text)",
            "rootcx_system.has_permission(uuid, text)",
        ] {
            exec(pool, &format!("REVOKE EXECUTE ON FUNCTION {sig} FROM PUBLIC")).await?;
        }

        // Retroactive RLS over tables that predate this refactor.
        let tables: Vec<(String, String)> = sqlx::query_as(
            "SELECT schemaname, tablename FROM pg_tables
             WHERE schemaname IN (SELECT id FROM rootcx_system.apps WHERE id <> 'core')",
        ).fetch_all(pool).await.map_err(RuntimeError::Schema)?;
        for (schema, table) in tables {
            apply_table_rls(pool, &schema, &table).await?;
        }

        info!("governance ready");
        Ok(())
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
