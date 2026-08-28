use std::collections::HashMap;

use sqlx::PgPool;
use tracing::info;
use uuid::Uuid;

use crate::RuntimeError;
use crate::data_types::{FieldType, FieldTypes, field_types, sql_default, system_field_types};
use crate::extensions::RuntimeExtension;
use rootcx_types::{AppManifest, EntityContract, FieldContract};

pub const SYSTEM_FIELDS: &[&str] = &["id", "created_at", "updated_at"];

#[inline]
pub fn is_system_field(name: &str) -> bool {
    SYSTEM_FIELDS.contains(&name)
}

pub async fn install_app(
    pool: &PgPool,
    manifest: &AppManifest,
    extensions: &[Box<dyn RuntimeExtension>],
    installed_by: Uuid,
    secrets: &crate::secrets::SecretManager,
) -> Result<(), RuntimeError> {
    validate_manifest(manifest)?;
    let app_id = &manifest.app_id;

    if !manifest.data_contract.is_empty() {
        let pk_types = build_pk_type_map(&manifest.data_contract);

        sqlx::query(&format!("CREATE SCHEMA IF NOT EXISTS {}", quote_ident(app_id)))
            .execute(pool)
            .await
            .map_err(RuntimeError::Schema)?;

        for entity in &manifest.data_contract {
            let ddl = generate_create_table(app_id, entity, &pk_types);
            sqlx::query(&ddl).execute(pool).await.map_err(RuntimeError::Schema)?;
            info!(table = %format!("{}.{}", app_id, entity.entity_name), "table ensured");
        }

        crate::schema_sync::sync_schema(pool, app_id, &manifest.data_contract, &pk_types).await?;
    }

    register_app(pool, manifest).await?;

    if !manifest.data_contract.is_empty() {
        for ext in extensions {
            for entity in &manifest.data_contract {
                ext.on_table_created(pool, manifest, app_id, &entity.entity_name).await?;
            }
        }

        for entity in &manifest.data_contract {
            let fk_statements = generate_foreign_keys(app_id, entity, &manifest.data_contract);
            for stmt in &fk_statements {
                sqlx::query(stmt).execute(pool).await.map_err(RuntimeError::Schema)?;
            }
            let table = &entity.entity_name;
            if let Some(key) = &entity.identity_key {
                let name = format!("idx_identity_{table}_{key}");
                create_index(pool, app_id, table, &name, key).await?;
            }
            // A confined caller's every query carries `owner = <me>`, so the column
            // needs an index or each read seq-scans the whole table. Named exactly
            // like the foreign-key index `generate_foreign_keys` emits: when the
            // owner is a referencing `entity_link` that index already exists and
            // this is a no-op, rather than a second index on the same column.
            if let Some(owner) = owner_field(entity) {
                let name = format!("idx_{app_id}_{table}_{owner}");
                create_index(pool, app_id, table, &name, owner).await?;
            }
            drop_orphaned_identity_indexes(pool, app_id, entity).await?;
        }
    }

    if !manifest.crons.is_empty() {
        crate::crons::sync_from_manifest(pool, app_id, &manifest.crons, Some(installed_by)).await?;
    }

    if !manifest.webhooks.is_empty() {
        crate::webhooks::sync_webhooks(pool, app_id, &manifest.webhooks, Some(installed_by), secrets).await?;
    }

    if !manifest.actions.is_empty() {
        sync_action_permissions(pool, app_id, &manifest.actions).await?;
    }

    for ext in extensions {
        ext.on_app_installed(pool, manifest, installed_by).await?;
    }

    info!(
        app = %app_id,
        entities = manifest.data_contract.len(),
        "app installed successfully"
    );

    Ok(())
}

/// A single-column index on an app table, created if absent. Every identifier is
/// quoted here rather than at each call site, so a caller cannot forget one.
async fn create_index(
    pool: &PgPool,
    app_id: &str,
    table: &str,
    name: &str,
    column: &str,
) -> Result<(), RuntimeError> {
    let sql = format!(
        "CREATE INDEX IF NOT EXISTS {} ON {}.{} ({})",
        quote_ident(name),
        quote_ident(app_id),
        quote_ident(table),
        quote_ident(column),
    );
    sqlx::query(&sql).execute(pool).await.map_err(RuntimeError::Schema)?;
    Ok(())
}

pub async fn uninstall_app(pool: &PgPool, app_id: &str) -> Result<(), RuntimeError> {
    crate::crons::delete_all_for_app(pool, app_id).await?;

    let drop_schema = format!("DROP SCHEMA IF EXISTS {} CASCADE", quote_ident(app_id));
    sqlx::query(&drop_schema).execute(pool).await.map_err(RuntimeError::Schema)?;

    sqlx::query("DELETE FROM rootcx_system.entity_hooks WHERE app_id = $1")
        .bind(app_id).execute(pool).await.map_err(RuntimeError::Schema)?;
    sqlx::query("DELETE FROM rootcx_system.sensitive_fields WHERE app_id = $1")
        .bind(app_id).execute(pool).await.map_err(RuntimeError::Schema)?;
    // The resolvers live in `rootcx_system`, so dropping the app's schema does not
    // take them with it. They would survive as callable descriptions of a table
    // that no longer exists.
    crate::extensions::rbac::prune_owner_resolvers(pool, app_id, &[]).await?;
    sqlx::query("DELETE FROM rootcx_system.secrets WHERE app_id = $1")
        .bind(app_id).execute(pool).await.map_err(RuntimeError::Schema)?;
    tokio::try_join!(
        sqlx::query("DELETE FROM pgmq.q_jobs WHERE message->>'app_id' = $1").bind(app_id).execute(pool),
        sqlx::query("DELETE FROM pgmq.a_jobs WHERE message->>'app_id' = $1").bind(app_id).execute(pool),
    ).map_err(RuntimeError::Schema)?;
    sqlx::query("DELETE FROM rootcx_system.apps WHERE id = $1")
        .bind(app_id).execute(pool).await.map_err(RuntimeError::Schema)?;

    info!(app = %app_id, "app uninstalled");
    Ok(())
}

fn generate_create_table(app_id: &str, entity: &EntityContract, pk_types: &HashMap<String, String>) -> String {
    let table_name = format!("{}.{}", quote_ident(app_id), quote_ident(&entity.entity_name));

    let mut columns: Vec<String> = Vec::new();

    let has_id = entity.fields.iter().any(|f| f.name == "id");
    if !has_id {
        columns.push("\"id\" UUID PRIMARY KEY DEFAULT gen_random_uuid()".to_string());
    }

    for field in &entity.fields {
        if field.name == "created_at" || field.name == "updated_at" {
            continue;
        }
        columns.push(field_to_column(field, pk_types));
    }

    columns.push("\"created_at\" TIMESTAMPTZ NOT NULL DEFAULT now()".to_string());
    columns.push("\"updated_at\" TIMESTAMPTZ NOT NULL DEFAULT now()".to_string());

    format!("CREATE TABLE IF NOT EXISTS {} (\n  {}\n)", table_name, columns.join(",\n  "))
}

fn generate_foreign_keys(app_id: &str, entity: &EntityContract, all_entities: &[EntityContract]) -> Vec<String> {
    let table_name = format!("{}.{}", quote_ident(app_id), quote_ident(&entity.entity_name));
    let mut stmts = Vec::new();

    for field in &entity.fields {
        if field.field_type != "entity_link" { continue; }
        let refs = match &field.references {
            Some(r) => r,
            None => continue,
        };

        let (target_table, pk_col, fk_suffix) = match parse_entity_ref(&refs.entity) {
            RefTarget::Local(ref target) => {
                if !all_entities.iter().any(|e| e.entity_name == *target) { continue; }
                (format!("{}.{}", quote_ident(app_id), quote_ident(target)), "id", target.clone())
            }
            RefTarget::Core(ref name) => {
                let Some((schema, tbl, pk, _)) = resolve_core_entity(name) else { continue };
                (format!("{}.{}", quote_ident(schema), quote_ident(tbl)), pk, format!("core_{name}"))
            }
            RefTarget::App { .. } => continue,
        };

        let on_delete = resolve_on_delete(field);

        let fk_name = format!("fk_{}_{}_{}_{}", app_id, entity.entity_name, field.name, fk_suffix);
        stmts.push(format!(
            "DO $$ BEGIN \
               ALTER TABLE {} ADD CONSTRAINT {} \
               FOREIGN KEY ({}) REFERENCES {}({}) ON DELETE {on_delete}; \
             EXCEPTION WHEN duplicate_object THEN NULL; \
             END $$",
            table_name, quote_ident(&fk_name), quote_ident(&field.name), target_table, quote_ident(pk_col)
        ));

        let idx_name = format!("idx_{}_{}_{}", app_id, entity.entity_name, field.name);
        stmts.push(format!(
            "CREATE INDEX IF NOT EXISTS {} ON {} ({})",
            quote_ident(&idx_name), table_name, quote_ident(&field.name)
        ));
    }

    stmts
}

pub(crate) fn build_pk_type_map(entities: &[EntityContract]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for entity in entities {
        let pk_field = entity.fields.iter().find(|f| f.is_primary_key.unwrap_or(false) || f.name == "id");
        let pg_type = match pk_field {
            Some(f) => FieldType::from_field(f)
                .expect("build_pk_type_map requires a validated manifest")
                .postgres_type(),
            None => "UUID".to_string(),
        };
        map.insert(entity.entity_name.clone(), pg_type);
    }
    // Include core entity PK types so field_to_column resolves correct types
    for entity in entities {
        for field in &entity.fields {
            if field.field_type != "entity_link" { continue; }
            if let Some(refs) = &field.references {
                if let RefTarget::Core(name) = parse_entity_ref(&refs.entity) {
                    if let Some((_, _, _, pk_type)) = resolve_core_entity(&name) {
                        map.insert(refs.entity.clone(), pk_type.to_string());
                    }
                }
            }
        }
    }
    map
}

fn field_to_column(field: &rootcx_types::FieldContract, pk_types: &HashMap<String, String>) -> String {
    let col_name = quote_ident(&field.name);
    let is_pk = field.is_primary_key.unwrap_or(false) || field.name == "id";
    let field_type = FieldType::from_field(field)
        .expect("field_to_column requires a validated manifest");

    let pg_type = if field.field_type == "entity_link" {
        if let Some(refs) = &field.references {
            pk_types.get(&refs.entity).cloned().unwrap_or_else(|| "UUID".into())
        } else {
            "UUID".into()
        }
    } else {
        field_type.postgres_type()
    };

    let mut parts = vec![format!("{col_name} {pg_type}")];

    if is_pk {
        parts.push("PRIMARY KEY".to_string());
        if pg_type == "UUID" {
            parts.push("DEFAULT gen_random_uuid()".to_string());
        }
    }

    if field.required && !is_pk {
        parts.push("NOT NULL".to_string());
    }

    if let Some(ref default_val) = field.default_value
        && !is_pk
            && let Some(default_sql) = sql_default(default_val, &field_type) {
                parts.push(format!("DEFAULT {default_sql}"));
            }

    let col_def = parts.join(" ");

    if let Some(ref enum_values) = field.enum_values
        && !enum_values.is_empty() {
            let values_list: Vec<String> = enum_values.iter().map(|v| format!("'{}'", v.replace('\'', "''"))).collect();
            return format!("{col_def} CHECK ({col_name} IN ({}))", values_list.join(", "));
        }

    col_def
}

async fn load_entity(
    pool: &PgPool,
    app_id: &str,
    entity: &str,
) -> Result<Option<EntityContract>, crate::RuntimeError> {
    let Some(json) = load_manifest_json(pool, app_id).await? else { return Ok(None) };
    let Ok(m) = serde_json::from_value::<AppManifest>(json) else { return Ok(None) };
    Ok(m.data_contract.into_iter().find(|e| e.entity_name == entity))
}

pub async fn entity_exists(pool: &PgPool, app_id: &str, entity: &str) -> Result<bool, crate::RuntimeError> {
    Ok(load_entity(pool, app_id, entity).await?.is_some())
}

/// The app's stored manifest as raw JSON, for handing back to its own worker at
/// boot (see `OutboundMessage::Discover`). `None` if the app has no manifest.
pub async fn load_manifest_json(
    pool: &PgPool,
    app_id: &str,
) -> Result<Option<serde_json::Value>, crate::RuntimeError> {
    Ok(sqlx::query_as::<_, (serde_json::Value,)>("SELECT manifest FROM rootcx_system.apps WHERE id = $1")
        .bind(app_id)
        .fetch_optional(pool)
        .await
        .map_err(crate::RuntimeError::Schema)?
        .map(|(j,)| j))
}

pub async fn field_type_map(
    pool: &PgPool,
    app_id: &str,
    entity: &str,
) -> Result<FieldTypes, crate::RuntimeError> {
    let Some(entity) = load_entity(pool, app_id, entity).await? else {
        return Ok(system_field_types());
    };
    field_types(&entity).map_err(crate::RuntimeError::Invalid)
}

pub async fn entity_identity(
    pool: &PgPool,
    app_id: &str,
    entity: &str,
) -> Result<Option<(String, String)>, crate::RuntimeError> {
    Ok(load_entity(pool, app_id, entity)
        .await?
        .and_then(|e| e.identity_kind.zip(e.identity_key)))
}

pub async fn find_entities_by_identity(
    pool: &PgPool,
    identity_kind: &str,
    exclude_app: Option<&str>,
) -> Result<Vec<(String, String, String)>, crate::RuntimeError> {
    let rows: Vec<(Option<serde_json::Value>,)> = match exclude_app {
        Some(app) => sqlx::query_as("SELECT manifest FROM rootcx_system.apps WHERE id != $1 AND manifest IS NOT NULL")
            .bind(app).fetch_all(pool).await,
        None => sqlx::query_as("SELECT manifest FROM rootcx_system.apps WHERE manifest IS NOT NULL")
            .fetch_all(pool).await,
    }.map_err(crate::RuntimeError::Schema)?;

    Ok(rows
        .into_iter()
        .filter_map(|(json,)| serde_json::from_value::<AppManifest>(json?).ok())
        .flat_map(|m| {
            let app_id = m.app_id;
            m.data_contract.into_iter().filter_map(move |e| {
                e.identity_kind.as_deref()
                    .filter(|k| *k == identity_kind)
                    .and(e.identity_key)
                    .map(|key| (app_id.clone(), e.entity_name, key))
            })
        })
        .collect())
}

pub fn identity_index_name(entity: &EntityContract) -> Option<String> {
    entity.identity_key.as_ref().map(|k| format!("idx_identity_{}_{}", entity.entity_name, k))
}

pub async fn list_identity_indexes(
    pool: &PgPool,
    app_id: &str,
    table: &str,
) -> Result<Vec<String>, RuntimeError> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT indexname FROM pg_indexes WHERE schemaname = $1 AND tablename = $2 AND indexname LIKE 'idx_identity_%'"
    )
    .bind(app_id)
    .bind(table)
    .fetch_all(pool)
    .await
    .map_err(RuntimeError::Schema)?;
    Ok(rows.into_iter().map(|(n,)| n).collect())
}

async fn drop_orphaned_identity_indexes(
    pool: &PgPool,
    app_id: &str,
    entity: &EntityContract,
) -> Result<(), RuntimeError> {
    let expected = identity_index_name(entity);
    for name in list_identity_indexes(pool, app_id, &entity.entity_name).await? {
        if expected.as_ref() != Some(&name) {
            sqlx::query(&format!("DROP INDEX IF EXISTS {}.{}", quote_ident(app_id), quote_ident(&name)))
                .execute(pool).await.map_err(RuntimeError::Schema)?;
        }
    }
    Ok(())
}

// ── Cross-app reference DSL ─────────────────────────────────────────
// "accounts"      → Local  (same app)
// "core:users"    → Core   (rootcx_system)
// "crm:contacts"  → App    (cross-app, Phase 2)

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RefTarget {
    Local(String),
    Core(String),
    App { app: String, entity: String },
}

pub(crate) fn parse_entity_ref(raw: &str) -> RefTarget {
    match raw.split_once(':') {
        Some(("core", e)) => RefTarget::Core(e.into()),
        Some((app, e)) => RefTarget::App { app: app.into(), entity: e.into() },
        None => RefTarget::Local(raw.into()),
    }
}

/// Resolve a `core:X` name to (schema, table, pk_column, pk_type).
pub(crate) fn resolve_core_entity(name: &str) -> Option<(&'static str, &'static str, &'static str, &'static str)> {
    match name {
        "users" => Some(("rootcx_system", "users", "id", "UUID")),
        _ => None,
    }
}

pub(crate) fn resolve_on_delete(field: &rootcx_types::FieldContract) -> &'static str {
    match field.on_delete {
        Some(rootcx_types::OnDeletePolicy::Cascade)  => "CASCADE",
        Some(rootcx_types::OnDeletePolicy::Restrict) => "RESTRICT",
        Some(rootcx_types::OnDeletePolicy::SetNull)  => "SET NULL",
        None if field.required => "RESTRICT",
        None                   => "SET NULL",
    }
}

pub fn quote_ident(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

pub fn quote_literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Reject identifiers that aren't valid unquoted PostgreSQL names.
fn validate_ident(value: &str, label: &str) -> Result<(), RuntimeError> {
    if !value.is_empty()
        && value.as_bytes()[0].is_ascii_lowercase()
        && value.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    {
        return Ok(());
    }
    Err(RuntimeError::Invalid(format!(
        "{label} '{value}' must be snake_case (lowercase letters, digits, underscores; start with a letter)"
    )))
}

/// Permission keys are CSV-encoded into the `rootcx.effective_perms` GUC
/// (sql_proxy). A comma would corrupt the list, so keys are restricted to a
/// charset that excludes it: `[a-z0-9_:.*]`. Validated at every ingestion door
/// (manifest install + role API), never at the encoding boundary.
pub fn validate_perm_key(key: &str) -> Result<(), String> {
    if key.is_empty()
        || !key.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b':' | b'.' | b'*'))
    {
        return Err(format!("permission key '{key}' must match [a-z0-9_:.*]"));
    }
    Ok(())
}

/// The column deciding who a row belongs to, when the entity declares one.
///
/// Either it holds the user id itself — an `entity_link` to `core:users` (typed
/// UUID, and it earns a foreign key and index for free), a bare `uuid`, or `text`
/// — or it links to another entity that carries the ownership (see
/// [`owner_parent`]). Text is not a concession: the bundled gmail, google_calendar
/// and imap_smtp integrations store `user_id` as text across nine tables, and
/// confining those per user is the same need. The generated policy casts the
/// *caller's* id to the column's type rather than the column to text, so each shape
/// stays indexable (see `rbac::owner_predicate`).
pub(crate) fn owner_field(entity: &EntityContract) -> Option<&str> {
    entity
        .fields
        .iter()
        .find(|f| f.owner)
        .map(|f| f.name.as_str())
}

/// The entity this one delegates ownership to, when its owner column is a link to
/// a sibling entity rather than a user id. `None` covers both "no owner declared"
/// and "owns directly", which is exactly the distinction the policy builder needs.
pub(crate) fn owner_parent(entity: &EntityContract) -> Option<&str> {
    let field = entity.fields.iter().find(|f| f.owner)?;
    let target = &field.references.as_ref()?.entity;
    (field.field_type == "entity_link" && matches!(parse_entity_ref(target), RefTarget::Local(_)))
        .then_some(target.as_str())
}

/// Every entity's ownership as the policy builder consumes it: the column, and the
/// sibling it defers to. Built once from the manifest at install and once from the
/// projection at boot, so both paths generate the same SQL from the same shape.
pub(crate) fn owner_map(entities: &[EntityContract]) -> crate::extensions::rbac::OwnerMap {
    entities
        .iter()
        .filter_map(|e| {
            let column = owner_field(e)?.to_string();
            Some((e.entity_name.clone(), (column, owner_parent(e).map(str::to_string))))
        })
        .collect()
}

/// How many entities a delegation chain may span. Each extra link is one more
/// resolver the planner must run per confined query, and a chain this long is
/// already a modelling smell — so it is a refusal at install rather than a
/// surprise in production.
pub(crate) const MAX_OWNER_CHAIN: usize = 4;

/// Delegated ownership must terminate in a real user id, and must do so in bounded
/// time. Cross-entity, so it cannot live in `validate_owner_field`.
///
/// Every failure here would otherwise surface as a broken *table*: a chain that
/// loops makes Postgres report `infinite recursion detected in policy` only when
/// the table is first queried, and until the manifest is fixed the table cannot be
/// read at all. A chain ending on an entity that owns nothing is quieter and worse
/// — the policies simply match nothing, which reads as an access bug.
fn validate_owner_chains(manifest: &AppManifest) -> Result<(), String> {
    let by_name: HashMap<&str, &EntityContract> =
        manifest.data_contract.iter().map(|e| (e.entity_name.as_str(), e)).collect();

    for entity in &manifest.data_contract {
        let Some(mut parent) = owner_parent(entity) else { continue };
        let mut chain = vec![entity.entity_name.as_str()];
        loop {
            if chain.contains(&parent) {
                chain.push(parent);
                return Err(format!(
                    "entity '{}' delegates ownership in a loop ({}); a chain must end on a \
                     column holding a user id",
                    entity.entity_name, chain.join(" -> "),
                ));
            }
            chain.push(parent);
            if chain.len() > MAX_OWNER_CHAIN {
                return Err(format!(
                    "entity '{}' delegates ownership through {} entities ({}); at most \
                     {MAX_OWNER_CHAIN} are allowed",
                    entity.entity_name, chain.len(), chain.join(" -> "),
                ));
            }
            // Anything on the chain but the first link is resolved by a generated
            // function whose name carries both identifiers. Truncation at Postgres's
            // 63-byte limit would collide two entities into one resolver, silently
            // handing one entity's rows the other's owners, so refuse the name here.
            let resolver = crate::extensions::rbac::owner_resolver_name(&manifest.app_id, parent);
            if resolver.len() > 63 {
                return Err(format!(
                    "entity '{parent}' of app '{}' needs an ownership resolver named \
                     '{resolver}', which exceeds PostgreSQL's 63-byte identifier limit; \
                     shorten the app or entity name",
                    manifest.app_id,
                ));
            }

            // The reference itself is validated with every other `entity_link`, so a
            // missing target cannot reach here.
            let target = by_name[parent];
            if owner_field(target).is_none() {
                return Err(format!(
                    "entity '{}' delegates ownership to '{parent}', which declares no owner \
                     field; mark the column that owns a '{parent}' row with \"owner\": true",
                    entity.entity_name,
                ));
            }
            match owner_parent(target) {
                Some(next) => parent = next,
                None => break,
            }
        }
    }
    Ok(())
}

fn validate_owner_field(entity: &EntityContract) -> Result<(), String> {
    let owners: Vec<&FieldContract> = entity.fields.iter().filter(|f| f.owner).collect();

    // Zero is the norm. Two would make "the owner" ambiguous, and silently picking
    // one decides who sees what — so refuse rather than guess.
    let [owner] = owners[..] else {
        if owners.is_empty() {
            return Ok(());
        }
        let names: Vec<&str> = owners.iter().map(|f| f.name.as_str()).collect();
        return Err(format!(
            "entity '{}' marks {} fields as owner ({}); exactly one column may own a row",
            entity.entity_name, owners.len(), names.join(", ")
        ));
    };

    // A user id is compared as-is against the caller's, so the column has to be
    // able to hold one. Anything else would build a policy matching no row, which
    // reads as an access bug rather than as the manifest mistake it is.
    if !matches!(owner.field_type.as_str(), "entity_link" | "uuid" | "text") {
        return Err(format!(
            "entity '{}': owner field '{}' is '{}'; must be entity_link, uuid or text to hold a user id",
            entity.entity_name, owner.name, owner.field_type
        ));
    }

    // Without a target there is no way to tell "holds a user id" from "defers to
    // the entity it links to", and the two generate opposite policies.
    if owner.field_type == "entity_link" && owner.references.is_none() {
        return Err(format!(
            "entity '{}': owner field '{}' is an entity_link with no 'references'; point it at \
             'core:users' to hold a user id, or at the entity that owns the row",
            entity.entity_name, owner.name,
        ));
    }
    Ok(())
}

/// Validate a key an *app* wants to declare. Stricter than `validate_perm_key`:
/// it also refuses the `.own` suffix, which the core mints for row-scoped keys.
///
/// The two are separate because they guard opposite directions. `.own` must be
/// *grantable* — a role holding `contacts.read.own` is the whole point — but not
/// *declarable*: the permission lattice reads `X.own` as strictly weaker than `X`
/// and narrows a delegated agent's authority along it, so an app free to declare
/// `billing` and `billing.own` as unrelated capabilities would make that
/// narrowing invent a grant nobody made. Reserving it at the declaration door
/// keeps the relation a fact about provenance while leaving the grant API open.
pub fn validate_declared_perm_key(key: &str) -> Result<(), String> {
    validate_perm_key(key)?;
    // Action segment only, so an app named `own_data` stays legal.
    let action = key.rsplit(':').next().unwrap_or(key);
    if action == "own" || action.ends_with(".own") {
        return Err(format!(
            "permission key '{key}': the '.own' suffix is reserved for core row-scoped keys"
        ));
    }
    Ok(())
}

fn validate_perm_key_schema(key: &str) -> Result<(), RuntimeError> {
    validate_declared_perm_key(key).map_err(RuntimeError::Invalid)
}

pub fn validate_manifest(manifest: &AppManifest) -> Result<(), RuntimeError> {
    validate_ident(&manifest.app_id, "appId")?;
    if let Some(perms) = &manifest.permissions {
        for p in &perms.permissions {
            validate_perm_key_schema(&p.key)?;
        }
    }
    for action in &manifest.actions {
        validate_perm_key_schema(&action.id)?;
    }
    let all_entity_names: Vec<&str> = manifest.data_contract.iter().map(|e| e.entity_name.as_str()).collect();
    for entity in &manifest.data_contract {
        validate_ident(&entity.entity_name, "entity name")?;
        for field in &entity.fields {
            validate_ident(&field.name, "field name")?;
            FieldType::from_field(field).map_err(RuntimeError::Invalid)?;
        }
        for field in &entity.fields {
            if field.field_type != "entity_link" { continue; }
            if let Some(refs) = &field.references {
                match parse_entity_ref(&refs.entity) {
                    RefTarget::Core(name) => {
                        if resolve_core_entity(&name).is_none() {
                            return Err(RuntimeError::Invalid(format!(
                                "field '{}' references 'core:{name}' — unknown core entity", field.name
                            )));
                        }
                    }
                    RefTarget::App { app, entity: ent } => {
                        return Err(RuntimeError::Invalid(format!(
                            "field '{}' references '{app}:{ent}' — cross-app references not yet supported", field.name
                        )));
                    }
                    RefTarget::Local(ref target) => {
                        if !all_entity_names.contains(&target.as_str()) {
                            return Err(RuntimeError::Invalid(format!(
                                "field '{}' references entity '{target}' which is not defined in this manifest", field.name
                            )));
                        }
                    }
                }
            }
        }

        validate_owner_field(entity).map_err(RuntimeError::Invalid)?;

        if let (Some(kind), Some(key)) = (&entity.identity_kind, &entity.identity_key) {
            validate_ident(kind, "identityKind")?;
            if !entity.fields.iter().any(|f| f.name == *key) {
                return Err(RuntimeError::Invalid(format!(
                    "identityKey '{key}' not found in fields of entity '{}'", entity.entity_name
                )));
            }
        } else if entity.identity_kind.is_some() != entity.identity_key.is_some() {
            return Err(RuntimeError::Invalid(
                "identityKind and identityKey must both be set or both be absent".into()
            ));
        }
    }
    validate_owner_chains(manifest).map_err(RuntimeError::Invalid)?;
    Ok(())
}

async fn sync_action_permissions(
    pool: &PgPool,
    app_id: &str,
    actions: &[rootcx_types::ActionDefinition],
) -> Result<(), RuntimeError> {
    let (keys, descs): (Vec<String>, Vec<String>) = actions.iter().map(|a| (
        format!("app:{app_id}:action:{}", a.id),
        format!("{} ({})", a.name, app_id),
    )).unzip();
    sqlx::query(
        "INSERT INTO rootcx_system.rbac_permissions (key, description)
         SELECT unnest($1::text[]), unnest($2::text[])
         ON CONFLICT (key) DO UPDATE SET description = EXCLUDED.description",
    )
    .bind(&keys)
    .bind(&descs)
    .execute(pool)
    .await
    .map_err(RuntimeError::Schema)?;
    Ok(())
}

async fn register_app(pool: &PgPool, manifest: &AppManifest) -> Result<(), RuntimeError> {
    let manifest_json = serde_json::to_value(manifest)
        .map_err(|e| RuntimeError::Schema(sqlx::Error::Protocol(e.to_string())))?;

    sqlx::query(
        r#"
        INSERT INTO rootcx_system.apps (id, name, version, status, manifest)
        VALUES ($1, $2, $3, 'installed', $4)
        ON CONFLICT (id) DO UPDATE SET
            name = EXCLUDED.name,
            version = EXCLUDED.version,
            manifest = EXCLUDED.manifest,
            updated_at = now()
        "#,
    )
    .bind(&manifest.app_id)
    .bind(&manifest.name)
    .bind(&manifest.version)
    .bind(&manifest_json)
    .execute(pool)
    .await
    .map_err(RuntimeError::Schema)?;

    info!(app = %manifest.app_id, "app registered");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rootcx_types::{EntityContract, FieldContract, FieldReference, OnDeletePolicy};
    use serde_json::json;

    fn field(name: &str, field_type: &str) -> FieldContract {
        FieldContract {
            name: name.to_string(),
            field_type: field_type.to_string(),
            precision: None,
            scale: None,
            required: false,
            default_value: None,
            enum_values: None,
            references: None,
            is_primary_key: None,
            on_delete: None,
            sensitive: false, owner: false,
        }
    }

    fn entity(name: &str, fields: Vec<FieldContract>) -> EntityContract {
        EntityContract { entity_name: name.to_string(), fields, identity_kind: None, identity_key: None, indexes: vec![], checks: vec![] }
    }

    /// `.own` marks a row-scoped key that `intersect_permissions` treats as
    /// strictly weaker than its base. An app declaring it as an unrelated
    /// capability would make a delegated agent narrow to the wrong one, so the
    /// suffix is refused at every ingestion door rather than merely discouraged.
    #[test]
    fn validate_perm_key_reserves_the_own_suffix() {
        for key in ["contacts.read.own", "billing.own", "app:crm:contacts.read.own", "own"] {
            let error = validate_declared_perm_key(key).expect_err(&format!("must reject '{key}'"));
            assert!(
                error.contains("reserved"),
                "expected a reservation error for '{key}', got: {error}"
            );
        }
        // The suffix is only reserved on the action segment, and only whole:
        // these remain legal keys.
        for key in ["contacts.read", "owner.read", "own_data.read", "app:crm:disown.read"] {
            assert!(
                validate_declared_perm_key(key).is_ok(),
                "'{key}' must stay valid: {:?}",
                validate_declared_perm_key(key)
            );
        }
    }

    /// Refuse, at install, any owner declaration no working policy can be built
    /// from. Both failures are silent if they reach the DDL: two owners make
    /// "mine" ambiguous so the generator picks one and quietly decides who sees
    /// what, and a type that cannot hold a user id yields a policy matching no
    /// row — which reads as an access bug, not as the manifest mistake it is.
    #[test]
    fn an_owner_declaration_no_policy_can_use_is_refused() {
        for field_type in ["entity_link", "uuid", "text"] {
            let mut owner = field("user_id", field_type);
            owner.owner = true;
            owner.references = (field_type == "entity_link")
                .then(|| FieldReference { entity: "core:users".into(), field: "id".into() });
            assert!(
                validate_owner_field(&entity("profile", vec![owner])).is_ok(),
                "'{field_type}' can hold a user id and must be accepted"
            );
        }

        let mut dangling = field("user_id", "entity_link");
        dangling.owner = true;
        let error = validate_owner_field(&entity("profile", vec![dangling]))
            .expect_err("a link with no target is neither a user id nor a delegation");
        assert!(error.contains("no 'references'"), "{error}");

        for field_type in ["number", "boolean", "json", "timestamp", "[text]"] {
            let mut owner = field("user_id", field_type);
            owner.owner = true;
            let error = validate_owner_field(&entity("profile", vec![owner]))
                .expect_err(&format!("'{field_type}' cannot hold a user id"));
            assert!(error.contains("must be entity_link, uuid or text"), "{field_type}: {error}");
        }

        let (mut a, mut b) = (field("user_id", "uuid"), field("assignee", "uuid"));
        (a.owner, b.owner) = (true, true);
        let error = validate_owner_field(&entity("profile", vec![a, b]))
            .expect_err("two owner columns leave 'mine' undefined");
        assert!(error.contains("exactly one column may own a row"), "{error}");
    }

    /// A chain of entities, each owned by the next, the last one directly. Built
    /// root-last so a slice of it is still a valid manifest.
    fn owned_chain(names: &[&str]) -> AppManifest {
        let entities = names.iter().enumerate().map(|(i, name)| {
            let mut owner = match names.get(i + 1) {
                Some(parent) => {
                    let mut link = field(&format!("{parent}_id"), "entity_link");
                    link.references = Some(FieldReference {
                        entity: (*parent).into(), field: "id".into(),
                    });
                    link
                }
                None => field("user_id", "uuid"),
            };
            owner.owner = true;
            entity(name, vec![owner])
        }).collect();
        AppManifest {
            app_id: "hr".into(), name: "hr".into(), version: "1.0.0".into(),
            data_contract: entities, ..serde_json::from_value(json!({"appId":"hr","name":"hr"})).unwrap()
        }
    }

    /// Delegated ownership resolves through the manifest, so every way it can fail
    /// to terminate has to be caught here. Left to the database, a loop surfaces as
    /// `infinite recursion detected in policy` the first time the table is read —
    /// after the deploy, and with the table unusable until the manifest is fixed.
    #[test]
    fn a_delegation_chain_must_terminate_and_stay_short() {
        for depth in 2..=MAX_OWNER_CHAIN {
            let names: Vec<String> = (0..depth).map(|i| format!("link_{i}")).collect();
            let names: Vec<&str> = names.iter().map(String::as_str).collect();
            assert!(
                validate_owner_chains(&owned_chain(&names)).is_ok(),
                "a chain of {depth} entities must be accepted: {:?}",
                validate_owner_chains(&owned_chain(&names)),
            );
        }

        let names: Vec<String> = (0..=MAX_OWNER_CHAIN).map(|i| format!("link_{i}")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let error = validate_owner_chains(&owned_chain(&names)).expect_err("too deep");
        assert!(error.contains("at most 4 are allowed"), "{error}");

        // The last entity of the chain drops its own owner: the chain now ends on
        // an entity that owns nothing, so no policy could ever match.
        let mut orphaned = owned_chain(&["assignment", "enrollment"]);
        orphaned.data_contract[1].fields[0].owner = false;
        let error = validate_owner_chains(&orphaned).expect_err("chain ends nowhere");
        assert!(error.contains("declares no owner field"), "{error}");

        // Two entities each claiming the other owns them.
        let mut loops = owned_chain(&["a", "b"]);
        let mut back = field("a_id", "entity_link");
        back.owner = true;
        back.references = Some(FieldReference { entity: "a".into(), field: "id".into() });
        loops.data_contract[1].fields = vec![back];
        let error = validate_owner_chains(&loops).expect_err("a loop never terminates");
        assert!(error.contains("in a loop"), "{error}");
    }

    /// The generated resolver carries both identifiers, and Postgres truncates a
    /// name past 63 bytes. Two entities colliding into one resolver would hand one
    /// entity's rows the other's owners, so the name is refused instead.
    #[test]
    fn a_resolver_name_that_would_be_truncated_is_refused() {
        let long = "e".repeat(60);
        let error = validate_owner_chains(&owned_chain(&["child", &long]))
            .expect_err("the parent's resolver name does not fit");
        assert!(error.contains("63-byte identifier limit"), "{error}");
    }

    #[test]
    fn quote_ident_wraps() {
        assert_eq!(quote_ident("users"), "\"users\"");
        assert_eq!(quote_ident("my_table_2"), "\"my_table_2\"");
    }

    #[test]
    fn validate_ident_accepts_snake_case() {
        for id in ["app", "my_app", "sdr_agent", "app123", "a"] {
            assert!(validate_ident(id, "test").is_ok(), "should accept: {id}");
        }
    }

    #[test]
    fn validate_ident_rejects_invalid() {
        for id in ["", "sdr-agent", "MyApp", "123app", "app id", "app;drop", "!@#"] {
            assert!(validate_ident(id, "test").is_err(), "should reject: {id}");
        }
    }

    #[test]
    fn perm_key_charset() {
        for ok in ["app:crm:contacts.read", "app:crm:*", "admin:db.query", "app:x:action:do_thing", "tool:browser"] {
            assert!(validate_perm_key(ok).is_ok(), "should accept: {ok}");
        }
        // Comma is the CSV separator — rejecting it is the whole point.
        for bad in ["app:crm,billing:read", "", "App:CRM", "app:crm:read ", "app:crm:réad", "app:crm:a;b"] {
            assert!(validate_perm_key(bad).is_err(), "should reject: {bad}");
        }
    }

    #[test]
    fn sql_defaults_follow_field_type() {
        assert_eq!(sql_default(&json!(null), &FieldType::Text), None);
        assert_eq!(sql_default(&json!(true), &FieldType::Boolean), Some("true".into()));
        assert_eq!(sql_default(&json!(42), &FieldType::Number), Some("42".into()));
        assert_eq!(sql_default(&json!("hello"), &FieldType::Text), Some("'hello'".into()));
        assert_eq!(sql_default(&json!("it's"), &FieldType::Text), Some("'it''s'".into()));
        assert_eq!(sql_default(&json!({"a": 1}), &FieldType::Text), None);

        let jsonb = sql_default(&json!({"a": 1}), &FieldType::Json).unwrap();
        assert!(jsonb.ends_with("::jsonb"), "expected ::jsonb suffix, got: {jsonb}");
    }

    #[test]
    fn build_pk_type_map_implicit_uuid() {
        let entities = vec![entity("contacts", vec![field("name", "text")])];
        let map = build_pk_type_map(&entities);
        assert_eq!(map.get("contacts").map(String::as_str), Some("UUID"));
    }

    #[test]
    fn build_pk_type_map_explicit_pk() {
        let mut id_field = field("id", "text");
        id_field.is_primary_key = Some(true);
        let entities = vec![entity("contacts", vec![id_field])];
        let map = build_pk_type_map(&entities);
        assert_eq!(map.get("contacts").map(String::as_str), Some("TEXT"));
    }

    #[test]
    fn field_to_column_simple_text() {
        let pk_types = HashMap::new();
        let col = field_to_column(&field("name", "text"), &pk_types);
        assert_eq!(col, "\"name\" TEXT");
    }

    #[test]
    fn field_to_column_required() {
        let pk_types = HashMap::new();
        let mut f = field("email", "text");
        f.required = true;
        let col = field_to_column(&f, &pk_types);
        assert!(col.contains("NOT NULL"), "expected NOT NULL in: {col}");
    }

    #[test]
    fn field_to_column_pk_uuid_default() {
        let pk_types = HashMap::new();
        let mut f = field("id", "entity_link");
        f.is_primary_key = Some(true);
        let col = field_to_column(&f, &pk_types);
        assert!(col.contains("PRIMARY KEY"), "expected PRIMARY KEY in: {col}");
        assert!(col.contains("gen_random_uuid()"), "expected gen_random_uuid() in: {col}");
    }

    #[test]
    fn field_to_column_with_default() {
        let pk_types = HashMap::new();
        let mut f = field("status", "text");
        f.default_value = Some(json!("N/A"));
        let col = field_to_column(&f, &pk_types);
        assert!(col.contains("DEFAULT 'N/A'"), "expected DEFAULT 'N/A' in: {col}");
    }

    #[test]
    fn field_to_column_with_enum() {
        let pk_types = HashMap::new();
        let mut f = field("color", "text");
        f.enum_values = Some(vec!["red".to_string(), "blue".to_string()]);
        let col = field_to_column(&f, &pk_types);
        assert!(col.contains("CHECK"), "expected CHECK in: {col}");
        assert!(col.contains("'red'"), "expected 'red' in: {col}");
        assert!(col.contains("'blue'"), "expected 'blue' in: {col}");
    }

    #[test]
    fn field_to_column_entity_link() {
        let mut pk_types = HashMap::new();
        pk_types.insert("accounts".to_string(), "UUID".to_string());
        let mut f = field("account_id", "entity_link");
        f.references = Some(FieldReference { entity: "accounts".to_string(), field: "id".to_string() });
        let col = field_to_column(&f, &pk_types);
        assert!(col.contains("UUID"), "expected UUID type in: {col}");
    }

    #[test]
    fn generate_create_table_structure() {
        let e = entity("contacts", vec![field("name", "text")]);
        let pk_types = build_pk_type_map(&[e.clone()]);
        let ddl = generate_create_table("myapp", &e, &pk_types);

        assert!(ddl.contains("\"myapp\".\"contacts\""), "expected qualified table name in: {ddl}");
        let first_col_line = ddl.lines().nth(1).expect("expected column lines");
        assert!(
            first_col_line.contains("UUID PRIMARY KEY"),
            "expected auto-id UUID PRIMARY KEY in first column: {first_col_line}"
        );
        assert!(ddl.contains("\"created_at\""), "expected created_at in: {ddl}");
        assert!(ddl.contains("\"updated_at\""), "expected updated_at in: {ddl}");
    }

    /// The bundled integrations are installed through the very validator this
    /// module exposes, so a manifest the validator refuses is an integration that
    /// cannot be deployed at all — and the failure only shows up in production, as
    /// a 400 with no log line. Peppol shipped `"type": "string"` for months that
    /// way. Nothing here mocks the manifests: they are read from `resources/`.
    #[test]
    fn every_bundled_integration_manifest_installs() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/integrations");
        let manifests: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
            .expect("resources/integrations must be readable")
            .map(|entry| entry.expect("readable dir entry").path().join("manifest.json"))
            .filter(|path| path.exists())
            .collect();

        // An empty list would make every assertion below vacuous, so a renamed
        // resources directory has to fail here rather than pass quietly.
        assert!(!manifests.is_empty(), "no bundled manifest found under {}", dir.display());

        for path in manifests {
            let raw = std::fs::read_to_string(&path).expect("readable manifest");
            let manifest: AppManifest = serde_json::from_str(&raw)
                .unwrap_or_else(|e| panic!("{}: manifest does not parse: {e}", path.display()));
            validate_manifest(&manifest)
                .unwrap_or_else(|e| panic!("{}: manifest would be refused at install: {e}", path.display()));
        }
    }

    #[test]
    fn generate_foreign_keys_basic() {
        let accounts = entity("accounts", vec![field("name", "text")]);
        let mut fk_field = field("account_id", "entity_link");
        fk_field.references = Some(FieldReference { entity: "accounts".to_string(), field: "id".to_string() });
        let deals = entity("deals", vec![fk_field]);
        let all = vec![accounts, deals.clone()];
        let stmts = generate_foreign_keys("myapp", &deals, &all);
        assert_eq!(stmts.len(), 2, "expected FK + index statements, got: {stmts:?}");
    }

    #[test]
    fn generate_foreign_keys_core_ref() {
        let mut fk_field = field("owner_id", "entity_link");
        fk_field.references = Some(FieldReference { entity: "core:users".to_string(), field: "id".to_string() });
        let tasks = entity("tasks", vec![fk_field]);
        let all = vec![tasks.clone()];
        let stmts = generate_foreign_keys("myapp", &tasks, &all);
        assert_eq!(stmts.len(), 2, "expected FK + index for core ref: {stmts:?}");
        assert!(stmts[0].contains("\"rootcx_system\".\"users\""), "FK should target rootcx_system.users: {}", stmts[0]);
    }

    #[test]
    fn generate_foreign_keys_on_delete_policy() {
        let accounts = entity("accounts", vec![field("name", "text")]);
        let cases: Vec<(bool, Option<OnDeletePolicy>, &str)> = vec![
            (false, None,                              "ON DELETE SET NULL"),
            (true,  None,                              "ON DELETE RESTRICT"),
            (true,  Some(OnDeletePolicy::Cascade),     "ON DELETE CASCADE"),
            (false, Some(OnDeletePolicy::Cascade),     "ON DELETE CASCADE"),
            (true,  Some(OnDeletePolicy::SetNull),     "ON DELETE SET NULL"),
            (true,  Some(OnDeletePolicy::Restrict),    "ON DELETE RESTRICT"),
        ];
        for (required, on_delete, expected_clause) in cases {
            let mut fk = field("account_id", "entity_link");
            fk.references = Some(FieldReference { entity: "accounts".into(), field: "id".into() });
            fk.required = required;
            fk.on_delete = on_delete;
            let child = entity("deals", vec![fk]);
            let all = vec![accounts.clone(), child.clone()];
            let stmts = generate_foreign_keys("myapp", &child, &all);
            assert!(
                stmts[0].contains(expected_clause),
                "required={required}, on_delete={on_delete:?}: expected '{expected_clause}' in: {}",
                stmts[0]
            );
        }
    }

    #[test]
    fn generate_foreign_keys_skips_cross_app() {
        let mut fk_field = field("contact_id", "entity_link");
        fk_field.references = Some(FieldReference { entity: "crm:contacts".to_string(), field: "id".to_string() });
        let tasks = entity("tasks", vec![fk_field]);
        let all = vec![tasks.clone()];
        let stmts = generate_foreign_keys("myapp", &tasks, &all);
        assert!(stmts.is_empty(), "cross-app refs should be skipped: {stmts:?}");
    }

    #[test]
    fn generate_foreign_keys_skips_unknown_local() {
        let mut fk_field = field("project_id", "entity_link");
        fk_field.references = Some(FieldReference { entity: "projects".to_string(), field: "id".to_string() });
        let tasks = entity("tasks", vec![fk_field]);
        let all = vec![tasks.clone()];
        let stmts = generate_foreign_keys("myapp", &tasks, &all);
        assert!(stmts.is_empty(), "unknown local refs should be skipped: {stmts:?}");
    }

    #[test]
    fn generate_foreign_keys_no_links() {
        let e = entity("notes", vec![field("body", "text")]);
        let all = vec![e.clone()];
        let stmts = generate_foreign_keys("myapp", &e, &all);
        assert!(stmts.is_empty(), "no entity_link fields should produce no FK stmts: {stmts:?}");
    }

    fn manifest_with(entities: Vec<EntityContract>) -> AppManifest {
        AppManifest {
            app_id: "testapp".into(),
            name: "Test".into(),
            version: "1.0.0".into(),
            description: String::new(),
            icon: None,
            app_type: Default::default(),
            permissions: None,
            data_contract: entities,
            actions: vec![],
            config_schema: None,
            user_auth: None,
            webhooks: vec![],
            instructions: None,
            trigger: None,
            crons: vec![],
            public: None,
        }
    }

    #[test]
    fn validate_identity_rejects_mismatched_kind_key() {
        let cases: Vec<(Option<&str>, Option<&str>, &str)> = vec![
            (Some("person"), None, "kind without key"),
            (None, Some("email"), "key without kind"),
        ];
        for (kind, key, label) in cases {
            let mut e = entity("contacts", vec![field("email", "text")]);
            e.identity_kind = kind.map(String::from);
            e.identity_key = key.map(String::from);
            let m = manifest_with(vec![e]);
            assert!(validate_manifest(&m).is_err(), "should reject: {label}");
        }
    }

    #[test]
    fn validate_identity_rejects_missing_field() {
        let mut e = entity("contacts", vec![field("name", "text")]);
        e.identity_kind = Some("person".into());
        e.identity_key = Some("email".into());
        let m = manifest_with(vec![e]);
        let err = validate_manifest(&m).unwrap_err().to_string();
        assert!(err.contains("identityKey 'email' not found"), "expected field-not-found error, got: {err}");
    }

    #[test]
    fn validate_identity_rejects_invalid_kind() {
        let mut e = entity("contacts", vec![field("email", "text")]);
        e.identity_kind = Some("My-Kind".into());
        e.identity_key = Some("email".into());
        let m = manifest_with(vec![e]);
        assert!(validate_manifest(&m).is_err(), "should reject non-snake_case identityKind");
    }

    #[test]
    fn validate_identity_accepts_valid() {
        let mut e = entity("contacts", vec![field("email", "text")]);
        e.identity_kind = Some("person".into());
        e.identity_key = Some("email".into());
        let m = manifest_with(vec![e]);
        assert!(validate_manifest(&m).is_ok());
    }

    #[test]
    fn validate_identity_accepts_absent() {
        let m = manifest_with(vec![entity("contacts", vec![field("name", "text")])]);
        assert!(validate_manifest(&m).is_ok());
    }

    #[test]
    fn validate_manifest_enforces_the_field_type_contract() {
        let mut price = field("price", "decimal");
        price.precision = Some(19);
        let error = validate_manifest(&manifest_with(vec![entity("products", vec![price])]))
            .expect_err("manifest validation must reject an incomplete decimal contract")
            .to_string();
        assert!(error.contains("precision and scale together"), "{error}");
    }

    #[test]
    fn parse_entity_ref_variants() {
        let cases: Vec<(&str, RefTarget)> = vec![
            ("accounts", RefTarget::Local("accounts".into())),
            ("core:users", RefTarget::Core("users".into())),
            ("crm:contacts", RefTarget::App { app: "crm".into(), entity: "contacts".into() }),
        ];
        for (input, expected) in cases {
            assert_eq!(parse_entity_ref(input), expected, "input: {input}");
        }
    }

    #[test]
    fn validate_rejects_invalid_entity_link_refs() {
        let cases: Vec<(&str, &str, &str)> = vec![
            ("core:nonexistent", "unknown core entity", "unknown core ref"),
            ("crm:contacts",     "not yet supported",  "cross-app ref"),
            ("projects",         "not defined",         "missing local ref"),
        ];
        for (ref_entity, expected_err, label) in cases {
            let mut f = field("ref_id", "entity_link");
            f.references = Some(FieldReference { entity: ref_entity.into(), field: "id".into() });
            let m = manifest_with(vec![entity("tasks", vec![f])]);
            let err = validate_manifest(&m).unwrap_err().to_string();
            assert!(err.contains(expected_err), "{label}: got {err}");
        }
    }

    #[test]
    fn validate_accepts_valid_entity_link_refs() {
        let mut core_ref = field("owner_id", "entity_link");
        core_ref.references = Some(FieldReference { entity: "core:users".into(), field: "id".into() });
        let mut local_ref = field("account_id", "entity_link");
        local_ref.references = Some(FieldReference { entity: "accounts".into(), field: "id".into() });
        let m = manifest_with(vec![
            entity("tasks", vec![core_ref, local_ref]),
            entity("accounts", vec![field("name", "text")]),
        ]);
        assert!(validate_manifest(&m).is_ok());
    }

    #[test]
    fn quote_literal_escapes_single_quotes() {
        let cases: Vec<(&str, &str)> = vec![
            ("hello", "'hello'"),
            ("it's", "'it''s'"),
            ("", "''"),
            ("a''b", "'a''''b'"),
            ("'; DROP TABLE x;--", "'''; DROP TABLE x;--'"),
        ];
        for (input, expected) in cases {
            assert_eq!(super::quote_literal(input), expected, "input: {input:?}");
        }
    }
}
