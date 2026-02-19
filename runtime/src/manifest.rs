use std::collections::HashMap;

use sqlx::PgPool;
use tracing::info;

use crate::extensions::RuntimeExtension;
use crate::RuntimeError;
use rootcx_shared_types::{AppManifest, EntityContract};

pub async fn install_app(
    pool: &PgPool,
    manifest: &AppManifest,
    extensions: &[Box<dyn RuntimeExtension>],
) -> Result<(), RuntimeError> {
    let app_id = &manifest.app_id;

    if manifest.data_contract.is_empty() {
        info!(app = %app_id, "no dataContract, skipping table creation");
        register_app(pool, manifest).await?;
        return Ok(());
    }

    let pk_types = build_pk_type_map(&manifest.data_contract);

    sqlx::query(&format!("CREATE SCHEMA IF NOT EXISTS {}", quote_ident(app_id)))
        .execute(pool)
        .await
        .map_err(RuntimeError::Schema)?;
    info!(app = %app_id, "schema created");

    for entity in &manifest.data_contract {
        let ddl = generate_create_table(app_id, entity, &pk_types);
        sqlx::query(&ddl)
            .execute(pool)
            .await
            .map_err(RuntimeError::Schema)?;

        info!(table = %format!("{}.{}", app_id, entity.entity_name), "table created");
    }

    for ext in extensions {
        for entity in &manifest.data_contract {
            ext.on_table_created(pool, manifest, app_id, &entity.entity_name).await?;
        }
    }

    for entity in &manifest.data_contract {
        let fk_statements = generate_foreign_keys(app_id, entity, &manifest.data_contract);
        for stmt in &fk_statements {
            sqlx::query(stmt)
                .execute(pool)
                .await
                .map_err(RuntimeError::Schema)?;
        }
    }

    register_app(pool, manifest).await?;

    for ext in extensions {
        ext.on_app_installed(pool, manifest).await?;
    }

    info!(
        app = %app_id,
        entities = manifest.data_contract.len(),
        "app installed successfully"
    );

    Ok(())
}

pub async fn uninstall_app(pool: &PgPool, app_id: &str) -> Result<(), RuntimeError> {
    let drop_schema = format!("DROP SCHEMA IF EXISTS {} CASCADE", quote_ident(app_id));
    sqlx::query(&drop_schema)
        .execute(pool)
        .await
        .map_err(RuntimeError::Schema)?;

    sqlx::query("DELETE FROM rootcx_system.apps WHERE id = $1")
        .bind(app_id)
        .execute(pool)
        .await
        .map_err(RuntimeError::Schema)?;

    info!(app = %app_id, "app uninstalled");
    Ok(())
}

fn generate_create_table(
    app_id: &str,
    entity: &EntityContract,
    pk_types: &HashMap<String, &'static str>,
) -> String {
    let table_name = format!("{}.{}", quote_ident(app_id), quote_ident(&entity.entity_name));

    let mut columns: Vec<String> = Vec::new();

    // Auto-add id if not defined
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

    // Always add system timestamps
    columns.push("\"created_at\" TIMESTAMPTZ NOT NULL DEFAULT now()".to_string());
    columns.push("\"updated_at\" TIMESTAMPTZ NOT NULL DEFAULT now()".to_string());

    format!(
        "CREATE TABLE IF NOT EXISTS {} (\n  {}\n)",
        table_name,
        columns.join(",\n  ")
    )
}

fn generate_foreign_keys(
    app_id: &str,
    entity: &EntityContract,
    all_entities: &[EntityContract],
) -> Vec<String> {
    let table_name = format!("{}.{}", quote_ident(app_id), quote_ident(&entity.entity_name));
    let mut stmts = Vec::new();

    for field in &entity.fields {
        if field.field_type != "entity_link" {
            continue;
        }

        let refs = match &field.references {
            Some(r) => r,
            None => continue,
        };

        // Skip system / cross-app references
        if refs.entity.starts_with("system.") {
            continue;
        }
        let is_local = all_entities.iter().any(|e| e.entity_name == refs.entity);
        if !is_local {
            continue;
        }

        let target_table = format!("{}.{}", quote_ident(app_id), quote_ident(&refs.entity));
        let fk_name = format!(
            "fk_{}_{}_{}_{}",
            app_id, entity.entity_name, field.name, refs.entity
        );

        // FK via DO block (idempotent)
        stmts.push(format!(
            "DO $$ BEGIN \
               ALTER TABLE {} ADD CONSTRAINT {} \
               FOREIGN KEY ({}) REFERENCES {}(\"id\") ON DELETE SET NULL; \
             EXCEPTION WHEN duplicate_object THEN NULL; \
             END $$",
            table_name,
            quote_ident(&fk_name),
            quote_ident(&field.name),
            target_table
        ));

        // Index on FK column
        let idx_name = format!("idx_{}_{}_{}", app_id, entity.entity_name, field.name);
        stmts.push(format!(
            "CREATE INDEX IF NOT EXISTS {} ON {} ({})",
            quote_ident(&idx_name),
            table_name,
            quote_ident(&field.name)
        ));
    }

    stmts
}

fn build_pk_type_map(entities: &[EntityContract]) -> HashMap<String, &'static str> {
    let mut map = HashMap::new();
    for entity in entities {
        let pk_field = entity.fields.iter().find(|f| {
            f.is_primary_key.unwrap_or(false) || f.name == "id"
        });
        let pg_type = match pk_field {
            Some(f) => map_field_type(&f.field_type),
            None => "UUID",
        };
        map.insert(entity.entity_name.clone(), pg_type);
    }
    map
}

fn field_to_column(field: &rootcx_shared_types::FieldContract, pk_types: &HashMap<String, &'static str>) -> String {
    let col_name = quote_ident(&field.name);
    let is_pk = field.is_primary_key.unwrap_or(false) || field.name == "id";

    let pg_type = if field.field_type == "entity_link" {
        if let Some(refs) = &field.references {
            pk_types.get(&refs.entity).copied().unwrap_or("UUID")
        } else {
            "UUID"
        }
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

    if let Some(ref default_val) = field.default_value {
        if !is_pk {
            if let Some(default_sql) = json_to_sql_default(default_val, pg_type) {
                parts.push(format!("DEFAULT {default_sql}"));
            }
        }
    }

    let col_def = parts.join(" ");

    if let Some(ref enum_values) = field.enum_values {
        if !enum_values.is_empty() {
            let values_list: Vec<String> = enum_values
                .iter()
                .map(|v| format!("'{}'", v.replace('\'', "''")))
                .collect();
            return format!(
                "{col_def} CHECK ({col_name} IN ({}))",
                values_list.join(", ")
            );
        }
    }

    col_def
}

fn map_field_type(field_type: &str) -> &'static str {
    match field_type {
        "text" => "TEXT",
        "number" => "DOUBLE PRECISION",
        "boolean" => "BOOLEAN",
        "date" => "DATE",
        "timestamp" => "TIMESTAMPTZ",
        "json" => "JSONB",
        "file" => "TEXT",
        "entity_link" => "UUID",
        "[text]" => "TEXT[]",
        "[number]" => "DOUBLE PRECISION[]",
        _ => "TEXT",
    }
}

fn json_to_sql_default(val: &serde_json::Value, pg_type: &str) -> Option<String> {
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

pub fn quote_ident(ident: &str) -> String {
    let sanitized: String = ident
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    format!("\"{}\"", sanitized)
}

async fn register_app(pool: &PgPool, manifest: &AppManifest) -> Result<(), RuntimeError> {
    let manifest_json = serde_json::to_value(manifest).unwrap_or_default();

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
    use rootcx_shared_types::{EntityContract, FieldContract, FieldReference};
    use serde_json::json;

    // ── helpers ──────────────────────────────────────────────────────

    fn field(name: &str, field_type: &str) -> FieldContract {
        FieldContract {
            name: name.to_string(),
            field_type: field_type.to_string(),
            required: false,
            default_value: None,
            enum_values: None,
            references: None,
            is_primary_key: None,
        }
    }

    fn entity(name: &str, fields: Vec<FieldContract>) -> EntityContract {
        EntityContract {
            entity_name: name.to_string(),
            fields,
        }
    }

    // ── quote_ident ─────────────────────────────────────────────────

    #[test]
    fn quote_ident_cases() {
        for (input, expected) in [
            ("users", "\"users\""),
            ("users; DROP TABLE--", "\"usersDROPTABLE\""),
            ("my_table_2", "\"my_table_2\""),
            ("", "\"\""),
            ("!@#$%^&*()", "\"\""),
        ] {
            assert_eq!(quote_ident(input), expected, "quote_ident({input:?})");
        }
    }

    // ── json_to_sql_default ─────────────────────────────────────────

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

    // ── build_pk_type_map ───────────────────────────────────────────

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

    // ── field_to_column ─────────────────────────────────────────────

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
        f.references = Some(FieldReference {
            entity: "accounts".to_string(),
            field: "id".to_string(),
        });
        let col = field_to_column(&f, &pk_types);
        assert!(col.contains("UUID"), "expected UUID type in: {col}");
    }

    // ── generate_create_table ───────────────────────────────────────

    #[test]
    fn generate_create_table_structure() {
        let e = entity("contacts", vec![field("name", "text")]);
        let pk_types = build_pk_type_map(&[e.clone()]);
        let ddl = generate_create_table("myapp", &e, &pk_types);

        assert!(
            ddl.contains("\"myapp\".\"contacts\""),
            "expected qualified table name in: {ddl}"
        );
        let first_col_line = ddl.lines().nth(1).expect("expected column lines");
        assert!(
            first_col_line.contains("UUID PRIMARY KEY"),
            "expected auto-id UUID PRIMARY KEY in first column: {first_col_line}"
        );
        assert!(ddl.contains("\"created_at\""), "expected created_at in: {ddl}");
        assert!(ddl.contains("\"updated_at\""), "expected updated_at in: {ddl}");
    }

    // ── generate_foreign_keys ───────────────────────────────────────

    #[test]
    fn generate_foreign_keys_basic() {
        let accounts = entity("accounts", vec![field("name", "text")]);
        let mut fk_field = field("account_id", "entity_link");
        fk_field.references = Some(FieldReference {
            entity: "accounts".to_string(),
            field: "id".to_string(),
        });
        let deals = entity("deals", vec![fk_field]);
        let all = vec![accounts, deals.clone()];
        let stmts = generate_foreign_keys("myapp", &deals, &all);
        assert_eq!(stmts.len(), 2, "expected FK + index statements, got: {stmts:?}");
    }

    #[test]
    fn generate_foreign_keys_skips_system() {
        let mut fk_field = field("owner_id", "entity_link");
        fk_field.references = Some(FieldReference {
            entity: "system.users".to_string(),
            field: "id".to_string(),
        });
        let deals = entity("deals", vec![fk_field]);
        let all = vec![deals.clone()];
        let stmts = generate_foreign_keys("myapp", &deals, &all);
        assert!(stmts.is_empty(), "system refs should be skipped: {stmts:?}");
    }

    #[test]
    fn generate_foreign_keys_skips_cross_app() {
        let mut fk_field = field("project_id", "entity_link");
        fk_field.references = Some(FieldReference {
            entity: "projects".to_string(),
            field: "id".to_string(),
        });
        // "projects" is NOT in all_entities
        let tasks = entity("tasks", vec![fk_field]);
        let all = vec![tasks.clone()];
        let stmts = generate_foreign_keys("myapp", &tasks, &all);
        assert!(stmts.is_empty(), "cross-app refs should be skipped: {stmts:?}");
    }

    #[test]
    fn generate_foreign_keys_no_links() {
        let e = entity("notes", vec![field("body", "text")]);
        let all = vec![e.clone()];
        let stmts = generate_foreign_keys("myapp", &e, &all);
        assert!(stmts.is_empty(), "no entity_link fields should produce no FK stmts: {stmts:?}");
    }
}
