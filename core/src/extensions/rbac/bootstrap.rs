//! RBAC schema & governance bootstrap (DDL, RLS roles, the SQL `has_permission`
//! function, role seeding, and one-shot migrations). Split out of `mod.rs` so the
//! extension adapter there stays the lifecycle/wiring, and all schema setup lives
//! in one place.

use sqlx::PgPool;
use tracing::info;

use crate::RuntimeError;
use super::{RbacExtension, exec};

impl RbacExtension {
    /// One-shot migration from per-app (old) to global (new) schema.
    pub(super) async fn migrate_to_global(&self, pool: &PgPool) -> Result<(), RuntimeError> {
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

    /// Flag stored keys ending on a suffix the core reserves for row scopes.
    ///
    /// The permission lattice reads such a suffix as "weaker than the base key" and
    /// narrows a delegated agent's authority along it, so an app that means
    /// something unrelated by it would have that narrowing resolve to the wrong
    /// capability. `validate_declared_perm_key` refuses every reserved suffix at
    /// each ingestion door from now on; this only reports keys already stored. A
    /// warning, not an error: refusing to boot would strand a tenant over a key
    /// that its next deploy rejects anyway.
    ///
    /// Matches the same shapes as the validator, off the same constant: the whole
    /// action segment, or its last dot-separated part. A key merely *containing* the
    /// word (`read.shared_inbox`) is not a scope key and must not be reported.
    pub(super) async fn warn_on_reserved_scope_keys(&self, pool: &PgPool) -> Result<(), RuntimeError> {
        let patterns: Vec<String> = crate::manifest::RESERVED_SCOPE_SUFFIXES.iter()
            .flat_map(|suffix| [suffix.to_string(), format!("%.{suffix}")])
            .collect();
        let reserved: Vec<String> = sqlx::query_scalar(
            "SELECT key FROM rootcx_system.rbac_permissions WHERE split_part(key, ':', 3) LIKE ANY($1)",
        ).bind(&patterns).fetch_all(pool).await.map_err(RuntimeError::Schema)?;

        if !reserved.is_empty() {
            tracing::warn!(
                keys = ?reserved, reserved = ?crate::manifest::RESERVED_SCOPE_SUFFIXES,
                "permission keys end on a suffix the core reserves for row-scoped keys; rename \
                 them before declaring row ownership on their entity, or delegated agents will \
                 narrow to the wrong key"
            );
        }
        Ok(())
    }

    /// Rename permission keys: tool.X → tool:X, integration.X.Y → integration:X:Y,
    /// {app}:X → app:{app}:X. Also updates role permission arrays.
    pub(super) async fn migrate_permission_keys(&self, pool: &PgPool) -> Result<(), RuntimeError> {
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
    pub(super) async fn bootstrap_governance(&self, pool: &PgPool) -> Result<(), RuntimeError> {
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

        // Reassert the executor's shape on upgrade too. CREATE ROLE IF ABSENT
        // alone would preserve privileges granted by a historical owner migration.
        exec(pool, "ALTER ROLE rootcx_app_executor NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOREPLICATION NOBYPASSRLS").await?;
        let memberships: Vec<String> = sqlx::query_scalar(
            "SELECT parent.rolname FROM pg_auth_members m
             JOIN pg_roles parent ON parent.oid = m.roleid
             JOIN pg_roles member ON member.oid = m.member
             WHERE member.rolname = 'rootcx_app_executor'",
        ).fetch_all(pool).await.map_err(RuntimeError::Schema)?;
        for role in memberships {
            exec(pool, &format!("REVOKE {} FROM rootcx_app_executor", crate::manifest::quote_ident(&role))).await?;
        }

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

        crate::governance::approved_actions::install_gate(pool).await?;

        // The normal function RLS policies call. Reads the identity GUCs posed
        // by the Core. Cross-app authority is intentionally not folded into
        // this predicate: an owned table must combine the grant with its row
        // ownership predicate instead of letting a permissive base policy OR
        // away the ownership check.
        exec(pool,
            "CREATE OR REPLACE FUNCTION rootcx_system.check_access(p_required TEXT)
             RETURNS BOOLEAN AS $$
             DECLARE v_user_id UUID; v_delegated TEXT; v_perms TEXT;
             BEGIN
                 IF coalesce(current_setting('rootcx.approved_action_id', true), '') <> '' THEN
                     RETURN rootcx_system.check_approved_action_access(p_required);
                 END IF;
                 IF coalesce(current_setting('rootcx.publication_id', true), '') <> '' THEN
                     RETURN FALSE;
                 END IF;
                 -- User permissions do not authorize a worker to leave its
                 -- Core-bound app. Authorized remote operations explicitly
                 -- open a provider-context transaction after grant checks.
                 IF left(p_required, 4) = 'app:' AND
                    coalesce(current_setting('rootcx.human_data_request', true), '') <> '1' AND
                    split_part(p_required, ':', 2) IS DISTINCT FROM
                        nullif(current_setting('rootcx.app_id', true), '') THEN
                     RETURN FALSE;
                 END IF;
                 IF left(p_required, 4) = 'app:' AND NOT EXISTS (
                     SELECT 1 FROM rootcx_system.app_installations
                      WHERE app_id = split_part(p_required, ':', 2) AND active
                 ) THEN
                     RETURN FALSE;
                 END IF;
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

        // Cross-app authority is a separate RLS predicate. The generated
        // CRUD policies combine it with the target's ownership expression;
        // this keeps a provider grant from becoming an ownership bypass while
        // still allowing grants on ordinary (non-owned) collections. Delegated
        // calls additionally retain their frozen effective-permission ceiling;
        // only nondelegated calls acquire action authority from the app grant.
        exec(pool,
            "CREATE OR REPLACE FUNCTION rootcx_system.check_cross_app_access(p_required TEXT)
             RETURNS BOOLEAN AS $$
             DECLARE v_grant UUID; v_source TEXT; v_target TEXT;
                     v_perms TEXT; v_required TEXT; v_permission TEXT; v_action TEXT;
             BEGIN
                 IF coalesce(current_setting('rootcx.approved_action_id', true), '') <> '' THEN
                     RETURN FALSE;
                 END IF;
                 IF coalesce(current_setting('rootcx.publication_id', true), '') <> '' THEN
                     RETURN FALSE;
                 END IF;
                 IF nullif(current_setting('rootcx.user_id', true), '') IS NULL THEN
                     RETURN FALSE;
                 END IF;
                 IF current_setting('rootcx.is_delegated', true) = '1' THEN
                     v_perms := current_setting('rootcx.effective_perms', true);
                     IF v_perms IS NULL OR v_perms = '' THEN RETURN FALSE; END IF;
                     v_permission := p_required;
                     v_action := current_setting('rootcx.cross_app_action', true);
                     -- Core's fixed mutation executor needs SELECT for
                     -- targeting and RETURNING. Use its pinned operation's
                     -- ceiling, preserving the owned-policy scope.
                     IF v_action IN ('create', 'update', 'delete') THEN
                         IF right(p_required, 5) = '.read' THEN
                             v_permission := left(p_required, length(p_required) - 5) || '.' || v_action;
                         ELSIF right(p_required, 9) = '.read.own' THEN
                             v_permission := left(p_required, length(p_required) - 9) || '.' || v_action || '.own';
                         END IF;
                     END IF;
                     IF NOT rootcx_system.match_permission(string_to_array(v_perms, ','), v_permission) THEN
                         RETURN FALSE;
                     END IF;
                 END IF;
                 BEGIN
                     v_grant := nullif(current_setting('rootcx.cross_app_grant_id', true), '')::uuid;
                 EXCEPTION WHEN invalid_text_representation THEN
                     RETURN FALSE;
                 END;
                 v_source := coalesce(nullif(current_setting('rootcx.cross_app_source_app', true), ''), '');
                 v_target := coalesce(nullif(current_setting('rootcx.app_id', true), ''), '');
                 -- Only owned-table policies call the .own entry point.
                 -- Check that exact delegated permission before normalizing
                 -- to the collection action stored on the app grant.
                 v_required := CASE WHEN p_required ~ '\\.(read|create|update|delete)\\.own$'
                                    THEN left(p_required, length(p_required) - 4)
                                    ELSE p_required END;
                 RETURN rootcx_system.cross_app_grant_allows(v_required, v_target, v_source, v_grant);
             END;
             $$ LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path = pg_catalog, rootcx_system",
        ).await?;

        // Only the Core's publication transaction constructor poses this key,
        // after locking and revalidating the exact publication and optional grant.
        exec(pool,
            "CREATE OR REPLACE FUNCTION rootcx_system.check_publication_access(p_required TEXT)
             RETURNS BOOLEAN LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $$
               SELECT EXISTS (
                 SELECT 1 FROM rootcx_system.publications p
                 JOIN rootcx_system.app_installations ci ON ci.id = p.consumer_installation_id AND ci.active
                 JOIN rootcx_system.app_installations pi ON pi.id = p.provider_installation_id AND pi.active
                 JOIN rootcx_system.public_execution_principals principal ON principal.installation_id = ci.id
                 JOIN rootcx_system.users u ON u.id = principal.user_id AND u.disabled_at IS NULL
                 WHERE p.id = nullif(current_setting('rootcx.publication_id', true), '')::uuid
                   AND u.id = nullif(current_setting('rootcx.user_id', true), '')::uuid
                   AND p.status = 'active'
                   AND (p.expires_at IS NULL OR p.expires_at > statement_timestamp())
                   AND p_required = current_setting('rootcx.publication_read_key', true)
                   AND p_required = 'app:' || p.provider_app || ':' || (p.definition->>'entity') || '.read'
                   AND p.provider_app = current_setting('rootcx.app_id', true)
                   AND (p.consumer_app = p.provider_app OR rootcx_system.cross_app_grant_allows(
                       p_required, p.provider_app, p.consumer_app,
                       nullif(current_setting('rootcx.cross_app_grant_id', true), '')::uuid))
               )
             $$",
        ).await?;

        // Lock down EXECUTE on the RBAC helpers. The two policy entry points
        // are check_access and check_cross_app_access; the helpers they call
        // run as SECURITY DEFINER owners, so the executor never needs direct
        // access to the RBAC tables. New functions default to EXECUTE by PUBLIC, so
        // without this an app could call resolve_permissions/has_permission with
        // an arbitrary user_id via ctx.sql and enumerate the whole RBAC graph
        // (violating Layer 2: "cannot read rootcx_system").
        for sig in [
            "rootcx_system.expand_roles(text[])",
            "rootcx_system.resolve_permissions(uuid)",
            "rootcx_system.match_permission(text[], text)",
            "rootcx_system.has_permission(uuid, text)",
            "rootcx_system.cross_app_grant_allows(text, text, text, uuid)",
            "rootcx_system.check_cross_app_access(text)",
            "rootcx_system.check_publication_access(text)",
        ] {
            exec(pool, &format!("REVOKE EXECUTE ON FUNCTION {sig} FROM PUBLIC")).await?;
        }
        exec(
            pool,
            "GRANT EXECUTE ON FUNCTION rootcx_system.check_cross_app_access(text),
                 rootcx_system.check_publication_access(text) TO rootcx_app_executor",
        )
        .await?;

        // Retroactive RLS across every app, so tables predating a refactor — and
        // any created by a deploy-time migration since the last boot — become
        // governed. Same rule the deploy path runs for one app; one implementation.
        super::govern_schema_tables(pool, None).await?;

        info!("governance ready");
        Ok(())
    }
}
