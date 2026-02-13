use std::collections::HashMap;

use sqlx::PgPool;
use tracing::info;

use crate::KernelError;
use rootcx_shared_types::{AppManifest, EntityContract, FieldContract};

// ── Public API ──────────────────────────────────────────────────────

/// Install an app by creating real SQL tables from its manifest dataContract.
///
/// For each entity: creates a schema per app, a table per entity, with proper
/// column types, constraints, indexes, and intra-app foreign keys.
///
/// Idempotent — uses `IF NOT EXISTS` throughout.
pub async fn install_app(pool: &PgPool, manifest: &AppManifest) -> Result<(), KernelError> {
    let app_id = &manifest.app_id;

    if manifest.data_contract.is_empty() {
        info!(app = %app_id, "no dataContract, skipping table creation");
        register_app(pool, manifest).await?;
        return Ok(());
    }

    let pk_types = build_pk_type_map(&manifest.data_contract);

    // 1. Create schema for this app
    let create_schema = format!("CREATE SCHEMA IF NOT EXISTS {}", quote_ident(app_id));
    sqlx::query(&create_schema)
        .execute(pool)
        .await
        .map_err(KernelError::Schema)?;

    info!(app = %app_id, "schema created");

    // 2. First pass: create all tables (without FK constraints)
    for entity in &manifest.data_contract {
        let ddl = generate_create_table(app_id, entity, &pk_types);
        sqlx::query(&ddl)
            .execute(pool)
            .await
            .map_err(KernelError::Schema)?;

        // Index on organization_id
        let idx = generate_org_index(app_id, entity);
        sqlx::query(&idx)
            .execute(pool)
            .await
            .map_err(KernelError::Schema)?;

        info!(table = %format!("{}.{}", app_id, entity.entity_name), "table created");
    }

    // 3. Second pass: add intra-app foreign keys
    for entity in &manifest.data_contract {
        let fk_statements = generate_foreign_keys(app_id, entity, &manifest.data_contract);
        for stmt in &fk_statements {
            sqlx::query(stmt)
                .execute(pool)
                .await
                .map_err(KernelError::Schema)?;
        }
    }

    // 4. Register in rootcx_system.apps
    register_app(pool, manifest).await?;

    info!(
        app = %app_id,
        entities = manifest.data_contract.len(),
        "app installed successfully"
    );

    Ok(())
}

/// Uninstall an app: drop its schema (CASCADE) and remove from rootcx_system.apps.
pub async fn uninstall_app(pool: &PgPool, app_id: &str) -> Result<(), KernelError> {
    let drop_schema = format!("DROP SCHEMA IF EXISTS {} CASCADE", quote_ident(app_id));
    sqlx::query(&drop_schema)
        .execute(pool)
        .await
        .map_err(KernelError::Schema)?;

    sqlx::query("DELETE FROM rootcx_system.apps WHERE id = $1")
        .bind(app_id)
        .execute(pool)
        .await
        .map_err(KernelError::Schema)?;

    info!(app = %app_id, "app uninstalled");
    Ok(())
}

// ── Pure SQL Generation (no DB, fully testable) ─────────────────────

/// Generate a `CREATE TABLE IF NOT EXISTS` statement from an entity contract.
pub fn generate_create_table(
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

    // Auto-add organization_id if not defined
    let has_org_id = entity.fields.iter().any(|f| f.name == "organization_id");
    if !has_org_id {
        columns.push("\"organization_id\" UUID NOT NULL".to_string());
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

/// Generate an index on organization_id for an entity table.
pub fn generate_org_index(app_id: &str, entity: &EntityContract) -> String {
    let table_name = format!("{}.{}", quote_ident(app_id), quote_ident(&entity.entity_name));
    let idx_name = format!("idx_{}_{}_org", app_id, entity.entity_name);
    format!(
        "CREATE INDEX IF NOT EXISTS {} ON {} (\"organization_id\")",
        quote_ident(&idx_name),
        table_name
    )
}

/// Generate FK constraint + index statements for entity_link fields referencing
/// entities within the same app.
///
/// Returns pairs of (ALTER TABLE + DO block, CREATE INDEX) statements.
pub fn generate_foreign_keys(
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

/// Build a map of entity_name → PG type of its primary key.
///
/// If an entity explicitly defines an `id` field with `isPrimaryKey: true`,
/// we use its manifest type. Otherwise the PK is UUID.
pub fn build_pk_type_map(entities: &[EntityContract]) -> HashMap<String, &'static str> {
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

/// Convert a manifest FieldContract to a SQL column definition string.
///
/// `pk_types` maps entity names to their PK SQL type, so entity_link columns
/// match the type of the referenced entity's primary key.
pub fn field_to_column(field: &FieldContract, pk_types: &HashMap<String, &'static str>) -> String {
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

    if let Some(ref validation) = field.validation {
        if let Some(ref enum_values) = validation.enum_values {
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
    }

    col_def
}

/// Map manifest field type strings to PostgreSQL types.
pub fn map_field_type(field_type: &str) -> &'static str {
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

/// Convert a serde_json Value to a SQL DEFAULT literal.
pub fn json_to_sql_default(val: &serde_json::Value, pg_type: &str) -> Option<String> {
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

/// Quote a SQL identifier. Only allows alphanumeric and underscore.
pub fn quote_ident(ident: &str) -> String {
    let sanitized: String = ident
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    format!("\"{}\"", sanitized)
}

// ── Private: DB-only helpers ────────────────────────────────────────

async fn register_app(pool: &PgPool, manifest: &AppManifest) -> Result<(), KernelError> {
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
    .map_err(KernelError::Schema)?;

    info!(app = %manifest.app_id, "app registered");
    Ok(())
}

