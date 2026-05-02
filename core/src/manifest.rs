use std::collections::HashMap;

use sqlx::PgPool;
use tracing::info;
use uuid::Uuid;

use crate::RuntimeError;
use crate::extensions::RuntimeExtension;
use rootcx_types::{AppManifest, EntityContract};

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
            if let Some(ref key) = entity.identity_key {
                let idx = format!(
                    "CREATE INDEX IF NOT EXISTS {} ON {}.{} ({})",
                    quote_ident(&format!("idx_identity_{}_{}", entity.entity_name, key)),
                    quote_ident(app_id),
                    quote_ident(&entity.entity_name),
                    quote_ident(key),
                );
                sqlx::query(&idx).execute(pool).await.map_err(RuntimeError::Schema)?;
            }
            drop_orphaned_identity_indexes(pool, app_id, entity).await?;
        }
    }

    if !manifest.crons.is_empty() {
        crate::crons::sync_from_manifest(pool, app_id, &manifest.crons, Some(installed_by)).await?;
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

pub async fn uninstall_app(pool: &PgPool, app_id: &str) -> Result<(), RuntimeError> {
    crate::crons::delete_all_for_app(pool, app_id).await?;

    let drop_schema = format!("DROP SCHEMA IF EXISTS {} CASCADE", quote_ident(app_id));
    sqlx::query(&drop_schema).execute(pool).await.map_err(RuntimeError::Schema)?;

    sqlx::query("DELETE FROM rootcx_system.entity_hooks WHERE app_id = $1")
        .bind(app_id).execute(pool).await.map_err(RuntimeError::Schema)?;
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

fn generate_create_table(app_id: &str, entity: &EntityContract, pk_types: &HashMap<String, &'static str>) -> String {
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

pub(crate) fn build_pk_type_map(entities: &[EntityContract]) -> HashMap<String, &'static str> {
    let mut map = HashMap::new();
    for entity in entities {
        let pk_field = entity.fields.iter().find(|f| f.is_primary_key.unwrap_or(false) || f.name == "id");
        let pg_type = match pk_field {
            Some(f) => map_field_type(&f.field_type),
            None => "UUID",
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
                        map.insert(refs.entity.clone(), pk_type);
                    }
                }
            }
        }
    }
    map
}

fn field_to_column(field: &rootcx_types::FieldContract, pk_types: &HashMap<String, &'static str>) -> String {
    let col_name = quote_ident(&field.name);
    let is_pk = field.is_primary_key.unwrap_or(false) || field.name == "id";

    let pg_type = if field.field_type == "entity_link" {
        if let Some(refs) = &field.references { pk_types.get(&refs.entity).copied().unwrap_or("UUID") } else { "UUID" }
    } else {
        map_field_type(&field.field_type)
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
            && let Some(default_sql) = json_to_sql_default(default_val, pg_type) {
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

pub fn map_field_type(field_type: &str) -> &'static str {
    match field_type {
        "text" => "TEXT",
        "number" => "DOUBLE PRECISION",
        "boolean" => "BOOLEAN",
        "date" => "DATE",
        "timestamp" => "TIMESTAMPTZ",
        "json" => "JSONB",
        "file" => "TEXT",
        "uuid" | "entity_link" => "UUID",
        "[text]" => "TEXT[]",
        "[number]" => "DOUBLE PRECISION[]",
        _ => "TEXT",
    }
}

pub(crate) fn json_to_sql_default(val: &serde_json::Value, pg_type: &str) -> Option<String> {
    match val {
        serde_json::Value::Null => None,
        serde_json::Value::Bool(b) => Some(b.to_string()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::String(s) => Some(format!("'{}'", s.replace('\'', "''"))),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            if pg_type == "JSONB" {
                Some(format!("'{}'::jsonb", val.to_string().replace('\'', "''")))
            } else {
                None
            }
        }
    }
}

async fn load_entity(
    pool: &PgPool,
    app_id: &str,
    entity: &str,
) -> Result<Option<EntityContract>, crate::RuntimeError> {
    let Some((json,)): Option<(serde_json::Value,)> =
        sqlx::query_as("SELECT manifest FROM rootcx_system.apps WHERE id = $1")
            .bind(app_id)
            .fetch_optional(pool)
            .await
            .map_err(crate::RuntimeError::Schema)?
    else {
        return Ok(None);
    };
    let Ok(m) = serde_json::from_value::<AppManifest>(json) else { return Ok(None) };
    Ok(m.data_contract.into_iter().find(|e| e.entity_name == entity))
}

pub async fn entity_exists(pool: &PgPool, app_id: &str, entity: &str) -> Result<bool, crate::RuntimeError> {
    Ok(load_entity(pool, app_id, entity).await?.is_some())
}

pub async fn field_type_map(
    pool: &PgPool,
    app_id: &str,
    entity: &str,
) -> Result<HashMap<String, String>, crate::RuntimeError> {
    let mut m: HashMap<String, String> = load_entity(pool, app_id, entity)
        .await?
        .map(|ec| ec.fields.iter().map(|f| (f.name.clone(), f.field_type.clone())).collect())
        .unwrap_or_default();
    m.insert("id".into(), "uuid".into());
    m.insert("created_at".into(), "timestamp".into());
    m.insert("updated_at".into(), "timestamp".into());
    Ok(m)
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
    Err(RuntimeError::Schema(sqlx::Error::Protocol(format!(
        "{label} '{value}' must be snake_case (lowercase letters, digits, underscores; start with a letter)"
    ))))
}

pub fn validate_manifest(manifest: &AppManifest) -> Result<(), RuntimeError> {
    validate_ident(&manifest.app_id, "appId")?;
    let all_entity_names: Vec<&str> = manifest.data_contract.iter().map(|e| e.entity_name.as_str()).collect();
    for entity in &manifest.data_contract {
        validate_ident(&entity.entity_name, "entity name")?;
        for field in &entity.fields {
            validate_ident(&field.name, "field name")?;
        }
        for field in &entity.fields {
            if field.field_type != "entity_link" { continue; }
            if let Some(refs) = &field.references {
                match parse_entity_ref(&refs.entity) {
                    RefTarget::Core(name) => {
                        if resolve_core_entity(&name).is_none() {
                            return Err(RuntimeError::Schema(sqlx::Error::Protocol(format!(
                                "field '{}' references 'core:{name}' — unknown core entity", field.name
                            ))));
                        }
                    }
                    RefTarget::App { app, entity: ent } => {
                        return Err(RuntimeError::Schema(sqlx::Error::Protocol(format!(
                            "field '{}' references '{app}:{ent}' — cross-app references not yet supported", field.name
                        ))));
                    }
                    RefTarget::Local(ref target) => {
                        if !all_entity_names.contains(&target.as_str()) {
                            return Err(RuntimeError::Schema(sqlx::Error::Protocol(format!(
                                "field '{}' references entity '{target}' which is not defined in this manifest", field.name
                            ))));
                        }
                    }
                }
            }
        }

        if let (Some(kind), Some(key)) = (&entity.identity_kind, &entity.identity_key) {
            validate_ident(kind, "identityKind")?;
            if !entity.fields.iter().any(|f| f.name == *key) {
                return Err(RuntimeError::Schema(sqlx::Error::Protocol(format!(
                    "identityKey '{key}' not found in fields of entity '{}'", entity.entity_name
                ))));
            }
        } else if entity.identity_kind.is_some() != entity.identity_key.is_some() {
            return Err(RuntimeError::Schema(sqlx::Error::Protocol(
                "identityKind and identityKey must both be set or both be absent".into()
            )));
        }
    }
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
            required: false,
            default_value: None,
            enum_values: None,
            references: None,
            is_primary_key: None,
            on_delete: None,
        }
    }

    fn entity(name: &str, fields: Vec<FieldContract>) -> EntityContract {
        EntityContract { entity_name: name.to_string(), fields, identity_kind: None, identity_key: None }
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
    fn json_to_sql_defaults() {
        assert_eq!(json_to_sql_default(&json!(null), "TEXT"), None);
        assert_eq!(json_to_sql_default(&json!(true), "BOOLEAN"), Some("true".into()));
        assert_eq!(json_to_sql_default(&json!(42), "DOUBLE PRECISION"), Some("42".into()));
        assert_eq!(json_to_sql_default(&json!("hello"), "TEXT"), Some("'hello'".into()));
        assert_eq!(json_to_sql_default(&json!("it's"), "TEXT"), Some("'it''s'".into()));
        assert_eq!(json_to_sql_default(&json!({"a": 1}), "TEXT"), None);

        let jsonb = json_to_sql_default(&json!({"a": 1}), "JSONB").unwrap();
        assert!(jsonb.ends_with("::jsonb"), "expected ::jsonb suffix, got: {jsonb}");
    }

    #[test]
    fn build_pk_type_map_implicit_uuid() {
        let entities = vec![entity("contacts", vec![field("name", "text")])];
        let map = build_pk_type_map(&entities);
        assert_eq!(map.get("contacts"), Some(&"UUID"));
    }

    #[test]
    fn build_pk_type_map_explicit_pk() {
        let mut id_field = field("id", "text");
        id_field.is_primary_key = Some(true);
        let entities = vec![entity("contacts", vec![id_field])];
        let map = build_pk_type_map(&entities);
        assert_eq!(map.get("contacts"), Some(&"TEXT"));
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
        pk_types.insert("accounts".to_string(), "UUID");
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
