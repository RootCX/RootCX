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
const ENTITY_ACTIONS: [&str; 4] = ["create", "read", "update", "delete"];

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

        self.warn_on_reserved_scope_keys(pool).await?;

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

        if let Some(c) = &manifest.permissions {
            for p in &c.permissions {
                keys.push(format!("app:{app_id}:{}", p.key));
                descs.push(p.description.clone());
            }
        }

        // Always generate the invoke permission so it is grantable per role
        keys.push(format!("app:{app_id}:invoke"));
        descs.push("invoke the app's agent".into());

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

        // Every table's policies are current by now (`on_table_created` ran for all
        // of them), so a resolver this manifest no longer delegates to has no
        // dependent left and is safe to drop.
        let delegated: Vec<String> = manifest.data_contract.iter()
            .filter_map(|e| crate::manifest::owner_parent(e).map(str::to_string))
            .collect();
        prune_owner_resolvers(pool, app_id, &delegated).await?;

        Ok(())
    }

    async fn on_table_created(
        &self,
        pool: &PgPool,
        manifest: &AppManifest,
        schema: &str,
        table: &str,
    ) -> Result<(), RuntimeError> {
        apply_table_rls(pool, schema, table, &crate::manifest::owner_map(&manifest.data_contract)).await
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

/// Apply RLS to every table that physically exists in an app schema, not just the
/// ones the manifest declares.
///
/// Two call sites, and they need the same rule for opposite reasons. Boot replays
/// it across all apps so tables predating a refactor become governed. Deploy runs
/// it for one app right after `app_migrations`, because a migration is the
/// documented way to create what the manifest cannot express — and
/// `apply_table_rls`'s GRANT is schema-wide with `ALTER DEFAULT PRIVILEGES`, so
/// such a table arrives with full CRUD for the sandboxed executor role and no RLS.
/// Authorization is only RLS, so that table would be readable and writable by
/// every user of the app until the next restart happened to sweep it up.
///
/// Ownership comes from the projection rather than the stored manifest, which is
/// never revalidated after install. Grouped per schema first: resolving a
/// delegated entity needs the entities it defers to, not only its own row.
pub(crate) async fn govern_schema_tables(
    pool: &PgPool,
    only: Option<&str>,
) -> Result<(), RuntimeError> {
    let tables: Vec<(String, String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT t.schemaname, t.tablename, s.owner_field, s.owner_parent
           FROM pg_tables t
           LEFT JOIN rootcx_system.sensitive_fields s
             ON s.app_id = t.schemaname AND s.entity = t.tablename
          WHERE t.schemaname IN (SELECT id FROM rootcx_system.apps WHERE id <> 'core')
            AND ($1::text IS NULL OR t.schemaname = $1)",
    )
    .bind(only)
    .fetch_all(pool)
    .await
    .map_err(RuntimeError::Schema)?;

    let mut owners: std::collections::HashMap<&str, OwnerMap> = std::collections::HashMap::new();
    for (schema, table, owner, parent) in &tables {
        if let Some(owner) = owner {
            owners.entry(schema)
                .or_default()
                .insert(table.clone(), (owner.clone(), parent.clone()));
        }
    }
    let empty = OwnerMap::new();
    for (schema, table, _, _) in &tables {
        apply_table_rls(pool, schema, table, owners.get(schema.as_str()).unwrap_or(&empty)).await?;
    }
    Ok(())
}

/// Enable + FORCE RLS on an app table, grant the restricted executor CRUD on it,
/// and (re)create its permission-gated policies. Idempotent: safe to call on every
/// install and on the retroactive boot pass. `schema`/`table` are validated
/// snake_case identifiers (see `manifest::validate_manifest`).
///
/// `owners` describes ownership for every entity of the schema, since resolving a
/// delegated one needs its ancestors too. A table absent from the map gets exactly
/// the pre-ownership SQL, so a schema that declares nothing is untouched.
pub(crate) async fn apply_table_rls(
    pool: &PgPool,
    schema: &str,
    table: &str,
    owners: &OwnerMap,
) -> Result<(), RuntimeError> {
    use crate::manifest::quote_ident;
    let qt = format!("{}.{}", quote_ident(schema), quote_ident(table));

    exec(pool, &format!("GRANT USAGE ON SCHEMA {} TO rootcx_app_executor", quote_ident(schema))).await?;
    exec(pool, &format!("GRANT SELECT, INSERT, UPDATE, DELETE ON {qt} TO rootcx_app_executor")).await?;
    exec(pool, &format!(
        "ALTER DEFAULT PRIVILEGES IN SCHEMA {} GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO rootcx_app_executor",
        quote_ident(schema),
    )).await?;
    exec(pool, &format!("ALTER TABLE {qt} ENABLE ROW LEVEL SECURITY")).await?;
    exec(pool, &format!("ALTER TABLE {qt} FORCE ROW LEVEL SECURITY")).await?;

    // "This row is mine", or None when there is no owner to compare against — in
    // which case every `_own` policy below is dropped and not recreated, so removing
    // the declaration removes the confinement instead of stranding it.
    let mine = owner_predicate(pool, schema, table, owners).await?;

    for (policy, command, action, clauses) in RLS_POLICIES {
        set_policy(
            pool, &qt, policy, command, clauses,
            Some(&gate(&format!("app:{schema}:{table}.{action}"))),
        ).await?;

        // The row-scoped twin. PERMISSIVE, so Postgres ORs it with the unscoped
        // policy above: access becomes `unscoped OR (scoped AND mine)`, which only
        // ever *adds* what a `.own` holder can reach and leaves every existing
        // grant bit-identical. RESTRICTIVE would AND instead, and lock every app
        // already in production out of its own data.
        let scoped = mine.as_ref().map(|mine| {
            let own_key = format!("app:{schema}:{table}.{action}.{}", crate::manifest::OWN_SCOPE);
            format!("{} AND {mine}", gate(&own_key))
        });
        set_policy(
            pool, &qt, &format!("{policy}_own"), command, clauses, scoped.as_deref(),
        ).await?;
    }

    Ok(())
}

/// The policies every app table carries, one per SQL command: the policy name,
/// the command, the permission action gating it, and the clauses it needs.
///
/// UPDATE is the only command taking its predicate twice — USING picks the rows it
/// may touch, WITH CHECK vets the row it leaves behind. Both are load-bearing for
/// a row-scoped policy: USING alone would still let a confined caller hand its own
/// row to somebody else.
const RLS_POLICIES: [(&str, &str, &str, &[&str]); 4] = [
    ("rootcx_rls_select", "SELECT", "read", &["USING"]),
    ("rootcx_rls_insert", "INSERT", "create", &["WITH CHECK"]),
    ("rootcx_rls_delete", "DELETE", "delete", &["USING"]),
    ("rootcx_rls_update", "UPDATE", "update", &["USING", "WITH CHECK"]),
];

/// A permission requirement as an RLS predicate. The `(SELECT ...)` wrapper makes
/// the planner evaluate `check_access` once per query (its arguments are constant)
/// instead of once per row — mandatory for perf, not cosmetic.
fn gate(key: &str) -> String {
    format!("(SELECT rootcx_system.check_access({}))", crate::manifest::quote_literal(key))
}

/// (Re)define one policy, or drop it when there is no predicate. Dropped first
/// either way, so every call is a redefinition rather than a duplicate — which is
/// what makes `apply_table_rls` safe to run on each install and on every boot.
async fn set_policy(
    pool: &PgPool,
    qt: &str,
    name: &str,
    command: &str,
    clauses: &[&str],
    predicate: Option<&str>,
) -> Result<(), RuntimeError> {
    exec(pool, &format!("DROP POLICY IF EXISTS {name} ON {qt}")).await?;
    let Some(predicate) = predicate else { return Ok(()) };
    let body = clauses
        .iter()
        .map(|clause| format!("{clause} ({predicate})"))
        .collect::<Vec<_>>()
        .join(" ");
    exec(pool, &format!("CREATE POLICY {name} ON {qt} FOR {command} {body}")).await
}

/// Ownership per entity of one schema: the owning column, and the sibling entity
/// that column defers to when ownership is delegated rather than held outright.
pub(crate) type OwnerMap = std::collections::HashMap<String, (String, Option<String>)>;

/// The `rootcx_system` function answering "which rows of this entity are the
/// caller's". Separated by `.`, which `validate_ident` bars from both halves, so no
/// pair of (schema, entity) can ever produce one name.
pub(crate) fn owner_resolver_name(schema: &str, entity: &str) -> String {
    format!("rootcx_own.{schema}.{entity}")
}

/// Whether PostgreSQL would store an identifier whole rather than truncate it at
/// `NAMEDATALEN - 1`. Counted in BYTES, which is what Postgres truncates on — the
/// boot pass reads names from the catalog, not from `validate_ident`, so a
/// multi-byte name is not structurally impossible here.
///
/// Load-bearing for resolver names specifically: two entities truncated to the same
/// name collapse into one function, and one entity's rows then answer with the
/// other's owners. That is a silent widening, not a fail-closed one, so it is
/// refused at install (`manifest::validate_owner_chains`, a clear deploy error) and
/// again where the name is actually created (`owner_predicate`, which fails closed).
pub(crate) fn fits_ident_limit(name: &str) -> bool { name.len() <= 63 }

/// "This row is mine", as SQL — or `None` when it cannot be answered.
///
/// For a directly-owned row it is one comparison. The caller's id is cast to the
/// column's type, never the column to text: `owner::text = $guc` is not indexable,
/// so on a `uuid` column it would turn every read by a confined caller into a
/// sequential scan. The type comes from the catalog, not the manifest — the boot
/// pass has only a column name, and the catalog is what the policy actually runs
/// against. As in `gate`, the `(SELECT ...)` wrapper keeps the GUC read an InitPlan:
/// once per query.
///
/// For a delegated row the answer lives in another table, and reaching it from
/// inside a policy has two hazards. Read it inline and Postgres applies *that*
/// table's policies to the subquery, so who owns a row would start depending on
/// what the caller may read, and a chain would recurse until Postgres refuses the
/// table outright. So each link is crossed through a `SECURITY DEFINER` resolver
/// (see `declare_owner_resolver`), which makes ownership a fact about the data
/// alone and cuts the recursion at a function boundary. Its result set is compared
/// with `= ANY (ARRAY(...))` rather than `IN (...)`: both are evaluated once per
/// query, but only the array form lets the planner drive the link column's index.
///
/// Anything unanswerable yields `None`, leaving the row-scoped policies absent —
/// and absent means deny, since a `.own` key grants nothing without a policy
/// honouring it. Fail-closed is how a projection that outlived its column, or one
/// hand-edited into a loop, stays survivable.
async fn owner_predicate(
    pool: &PgPool,
    schema: &str,
    table: &str,
    owners: &OwnerMap,
) -> Result<Option<String>, RuntimeError> {
    // Walk to the entity that holds a real user id. Bounded and loop-checked here
    // as well as at install: the boot pass replays a projection nobody revalidates,
    // and Postgres reports policy recursion only once the table is queried — by
    // which time the table is unusable.
    let mut chain: Vec<(&str, &str)> = Vec::new();
    let mut current = table;
    loop {
        let Some((column, parent)) = owners.get(current) else {
            if !chain.is_empty() {
                tracing::warn!(%schema, %table, %current, "ownership is delegated to an entity that declares none; row-scoped policies not created");
            }
            return Ok(None);
        };
        if chain.iter().any(|(entity, _)| *entity == current) {
            tracing::warn!(%schema, %table, %current, "ownership delegation loops; row-scoped policies not created");
            return Ok(None);
        }
        chain.push((current, column.as_str()));
        let Some(parent) = parent else { break };
        if chain.len() >= crate::manifest::MAX_OWNER_CHAIN {
            tracing::warn!(%schema, %table, "ownership delegation is too deep; row-scoped policies not created");
            return Ok(None);
        }
        current = parent.as_str();
    }

    let (root, root_column) = chain[chain.len() - 1];
    let Some(root_type) = column_type(pool, schema, root, root_column).await? else {
        tracing::warn!(
            %schema, table = %root, column = %root_column,
            "owner column is missing; row-scoped policies not created (holders of the \
             '.own' keys stay denied until the next deploy adds the column)"
        );
        return Ok(None);
    };
    let mut mine = format!(
        "{} = (SELECT nullif(current_setting('rootcx.user_id', true), ''))::{root_type}",
        crate::manifest::quote_ident(root_column),
    );

    // Descend back towards `table`, materialising one resolver per link crossed.
    // Idempotent, and done here rather than in a pass of its own so a child's
    // policy can never be created before the resolver it names exists.
    for index in (0..chain.len() - 1).rev() {
        let (parent, _) = chain[index + 1];
        let (entity, link) = chain[index];
        let (Some(pk), Some(_link_type)) = (
            primary_key(pool, schema, parent).await?,
            column_type(pool, schema, entity, link).await?,
        ) else {
            tracing::warn!(%schema, %entity, %link, %parent, "the delegation link or its target's primary key is missing; row-scoped policies not created");
            return Ok(None);
        };
        let Some(pk_type) = column_type(pool, schema, parent, &pk).await? else {
            tracing::warn!(%schema, %parent, %pk, "the delegation target's primary key has no type in the catalog; row-scoped policies not created");
            return Ok(None);
        };
        let resolver = owner_resolver_name(schema, parent);
        if !fits_ident_limit(&resolver) {
            tracing::warn!(%schema, %parent, %resolver, "the ownership resolver's name exceeds PostgreSQL's 63-byte identifier limit and would be truncated onto another entity's; row-scoped policies not created");
            return Ok(None);
        }
        declare_owner_resolver(pool, schema, parent, &resolver, &pk, &pk_type, &mine).await?;
        mine = format!(
            "{} = ANY (ARRAY(SELECT rootcx_system.{}()))",
            crate::manifest::quote_ident(link),
            crate::manifest::quote_ident(&resolver),
        );
    }

    Ok(Some(mine))
}

/// The set of primary keys of `entity` the caller owns.
///
/// `SECURITY DEFINER`, so it runs as the core role and reads the parent table
/// unfiltered. That is the whole point: ownership must be a property of the data,
/// not of the caller's grants on the tables the chain passes through, or a caller
/// holding `child.read.own` would see a different set of rows depending on whether
/// it also held `parent.read`.
///
/// Which is why it also checks `rootcx.app_id`. An RLS predicate is evaluated as
/// the *invoking* role, so `rootcx_app_executor` must hold EXECUTE — and an app's
/// `ctx.sql` runs as that same role. Without the guard, any app could call another
/// app's resolver and enumerate the caller's row ids there, which apps being
/// mutually untrusted is exactly what must not happen. The GUC is posed by
/// `set_rls_context` before the drop to the executor, and `set_config` is revoked
/// from that role, so an app cannot claim to be another.
///
/// `coalesce` makes an unset GUC pass rather than deny. Every path that evaluates
/// RLS at all goes through `begin_app_tx` — the single `SET LOCAL ROLE
/// rootcx_app_executor` in the codebase — so unset means the core's own superuser
/// pool, which bypasses RLS and never reaches a policy anyway. Fail-closed there
/// would buy nothing and would strand any future caller that reads an app table
/// directly. `nullif` is part of that: a pooled connection that once served an app
/// keeps the GUC as `''` rather than unset, and `''` means the same "nobody said"
/// as absent.
async fn declare_owner_resolver(
    pool: &PgPool,
    schema: &str,
    entity: &str,
    resolver: &str,
    pk: &str,
    pk_type: &str,
    mine: &str,
) -> Result<(), RuntimeError> {
    use crate::manifest::{quote_ident, quote_literal};
    let signature = format!("rootcx_system.{}()", quote_ident(resolver));
    let own_app = quote_literal(schema);
    // STABLE, never IMMUTABLE: the answer depends on the caller's GUCs and on the
    // table, so an IMMUTABLE marking would let the planner constant-fold one
    // caller's reachable set into a cached plan and serve it to every other user —
    // a permanent cross-user leak. `resolvers_are_stable_secdef_and_not_public`
    // (tests/row_ownership_test.rs) asserts the volatility in the catalog.
    exec(pool, &format!(
        "CREATE OR REPLACE FUNCTION {signature} RETURNS SETOF {pk_type} \
         LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $rootcx$ \
         SELECT {} FROM {}.{} \
          WHERE coalesce(nullif(current_setting('rootcx.app_id', true), ''), {own_app}) = {own_app} \
            AND {mine} $rootcx$",
        quote_ident(pk), quote_ident(schema), quote_ident(entity),
    )).await?;
    // A new function is executable by PUBLIC by default, and this one names a
    // specific user's rows.
    exec(pool, &format!("REVOKE ALL ON FUNCTION {signature} FROM PUBLIC")).await?;
    exec(pool, &format!("GRANT EXECUTE ON FUNCTION {signature} TO rootcx_app_executor")).await
}

/// Drop the schema's resolvers for entities no longer named in `keep`. Called once
/// the app's policies are current, so nothing dropped can still be referenced.
pub(crate) async fn prune_owner_resolvers(
    pool: &PgPool,
    schema: &str,
    keep: &[String],
) -> Result<(), RuntimeError> {
    let existing: Vec<String> = sqlx::query_scalar(
        "SELECT p.proname FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace \
          WHERE n.nspname = 'rootcx_system' AND p.proname LIKE 'rootcx\\_own.%'",
    ).fetch_all(pool).await.map_err(RuntimeError::Schema)?;

    for name in existing {
        let Some((owner_schema, entity)) = name["rootcx_own.".len()..].split_once('.') else { continue };
        if owner_schema != schema || keep.iter().any(|k| k == entity) { continue }
        exec(pool, &format!(
            "DROP FUNCTION IF EXISTS rootcx_system.{}()", crate::manifest::quote_ident(&name),
        )).await?;
    }
    Ok(())
}

async fn column_type(
    pool: &PgPool,
    schema: &str,
    table: &str,
    column: &str,
) -> Result<Option<String>, RuntimeError> {
    Ok(sqlx::query_scalar::<_, String>(
        "SELECT format_type(a.atttypid, a.atttypmod) FROM pg_attribute a
          WHERE a.attrelid = to_regclass($1) AND a.attname = $2 AND a.attnum > 0 AND NOT a.attisdropped",
    )
    .bind(qualified(schema, table))
    .bind(column)
    .fetch_optional(pool).await.map_err(RuntimeError::Schema)?)
}

/// The single-column primary key a delegation link points at. Read from the
/// catalog rather than assumed to be `id`, since an entity may name its own.
async fn primary_key(pool: &PgPool, schema: &str, table: &str) -> Result<Option<String>, RuntimeError> {
    Ok(sqlx::query_scalar::<_, String>(
        "SELECT a.attname FROM pg_index i
           JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = i.indkey[0]
          WHERE i.indrelid = to_regclass($1) AND i.indisprimary AND i.indnatts = 1",
    )
    .bind(qualified(schema, table))
    .fetch_optional(pool).await.map_err(RuntimeError::Schema)?)
}

fn qualified(schema: &str, table: &str) -> String {
    format!("{}.{}", crate::manifest::quote_ident(schema), crate::manifest::quote_ident(table))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every minted key must have a policy enforcing it, and every policy a key
    /// that reaches it. The two lists exist separately because one maps SQL
    /// commands and the other names permissions, so nothing but this stops a
    /// fifth action being added to one and silently missing from the other.
    #[test]
    fn minted_actions_match_the_policies() {
        let mut minted = ENTITY_ACTIONS;
        let mut gated = RLS_POLICIES.map(|(_, _, action, _)| action);
        minted.sort_unstable();
        gated.sort_unstable();
        assert_eq!(minted, gated);
    }
}
