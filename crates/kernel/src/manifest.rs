use std::collections::HashMap;

use sqlx::PgPool;
use tracing::info;

use crate::KernelError;
use rootcx_shared_types::{AppManifest, EntityContract, FieldContract};

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

    // Build a map of entity_name → PK SQL type (used to resolve entity_link column types)
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
        create_entity_table(pool, app_id, entity, &pk_types).await?;
    }

    // 3. Second pass: add intra-app foreign keys
    for entity in &manifest.data_contract {
        add_foreign_keys(pool, app_id, entity, &manifest.data_contract).await?;
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

/// Build a map of entity_name → PG type of its primary key.
///
/// If an entity explicitly defines an `id` field with `isPrimaryKey: true`,
/// we use its manifest type (e.g. "text" → TEXT). Otherwise the PK is UUID.
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

/// Create a single entity table with columns, types, defaults, and CHECK constraints.
async fn create_entity_table(
    pool: &PgPool,
    app_id: &str,
    entity: &EntityContract,
    pk_types: &HashMap<String, &'static str>,
) -> Result<(), KernelError> {
    let table_name = format!("{}.{}", quote_ident(app_id), quote_ident(&entity.entity_name));

    let mut columns: Vec<String> = Vec::new();

    // Check if the entity defines its own 'id' field
    let has_id_field = entity.fields.iter().any(|f| f.name == "id");
    if !has_id_field {
        columns.push("\"id\" UUID PRIMARY KEY DEFAULT gen_random_uuid()".to_string());
    }

    // Check if entity defines its own organization_id
    let has_org_id = entity.fields.iter().any(|f| f.name == "organization_id");
    if !has_org_id {
        columns.push("\"organization_id\" UUID NOT NULL".to_string());
    }

    for field in &entity.fields {
        // Skip system timestamp columns — we always add them at the end
        if field.name == "created_at" || field.name == "updated_at" {
            continue;
        }

        let col_def = field_to_column(field, pk_types);
        columns.push(col_def);
    }

    // Always add system timestamps
    columns.push("\"created_at\" TIMESTAMPTZ NOT NULL DEFAULT now()".to_string());
    columns.push("\"updated_at\" TIMESTAMPTZ NOT NULL DEFAULT now()".to_string());

    let create_table = format!(
        "CREATE TABLE IF NOT EXISTS {} (\n  {}\n)",
        table_name,
        columns.join(",\n  ")
    );

    sqlx::query(&create_table)
        .execute(pool)
        .await
        .map_err(KernelError::Schema)?;

    // Create index on organization_id
    let idx_name = format!("idx_{}_{}_org", app_id, entity.entity_name);
    let create_idx = format!(
        "CREATE INDEX IF NOT EXISTS {} ON {} (\"organization_id\")",
        quote_ident(&idx_name),
        table_name
    );
    sqlx::query(&create_idx)
        .execute(pool)
        .await
        .map_err(KernelError::Schema)?;

    info!(table = %table_name, "table created");
    Ok(())
}

/// Add foreign key constraints for entity_link fields that reference entities within the same app.
async fn add_foreign_keys(
    pool: &PgPool,
    app_id: &str,
    entity: &EntityContract,
    all_entities: &[EntityContract],
) -> Result<(), KernelError> {
    let table_name = format!("{}.{}", quote_ident(app_id), quote_ident(&entity.entity_name));

    for field in &entity.fields {
        if field.field_type != "entity_link" {
            continue;
        }

        let refs = match &field.references {
            Some(r) => r,
            None => continue,
        };

        // Skip system references (e.g. "system.organization_members")
        if refs.entity.starts_with("system.") {
            continue;
        }

        // Only create FK for entities that exist in this app's dataContract
        let target_entity = &refs.entity;
        let is_local = all_entities.iter().any(|e| e.entity_name == *target_entity);

        if !is_local {
            continue;
        }

        let fk_name = format!(
            "fk_{}_{}_{}_{}",
            app_id, entity.entity_name, field.name, target_entity
        );
        let target_table = format!("{}.{}", quote_ident(app_id), quote_ident(target_entity));

        let add_fk = format!(
            "DO $$ BEGIN \
               ALTER TABLE {} ADD CONSTRAINT {} \
               FOREIGN KEY ({}) REFERENCES {}(\"id\") ON DELETE SET NULL; \
             EXCEPTION WHEN duplicate_object THEN NULL; \
             END $$",
            table_name,
            quote_ident(&fk_name),
            quote_ident(&field.name),
            target_table
        );

        sqlx::query(&add_fk)
            .execute(pool)
            .await
            .map_err(KernelError::Schema)?;

        // Index on FK column for join performance
        let idx_name = format!("idx_{}_{}_{}", app_id, entity.entity_name, field.name);
        let create_idx = format!(
            "CREATE INDEX IF NOT EXISTS {} ON {} ({})",
            quote_ident(&idx_name),
            table_name,
            quote_ident(&field.name)
        );
        sqlx::query(&create_idx)
            .execute(pool)
            .await
            .map_err(KernelError::Schema)?;
    }

    Ok(())
}

/// Register the app in the rootcx_system.apps table.
async fn register_app(pool: &PgPool, manifest: &AppManifest) -> Result<(), KernelError> {
    let manifest_json = serde_json::to_value(manifest).unwrap_or_default();
    let entities: Vec<String> = manifest
        .data_contract
        .iter()
        .map(|e| e.entity_name.clone())
        .collect();

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

    info!(
        app = %manifest.app_id,
        entities = ?entities,
        "app registered in rootcx_system.apps"
    );

    Ok(())
}

// ── Field → Column Mapping ──────────────────────────────────────────

/// Convert a manifest FieldContract to a SQL column definition.
///
/// `pk_types` maps entity names to their PK SQL type, so entity_link columns
/// match the type of the referenced entity's primary key.
fn field_to_column(field: &FieldContract, pk_types: &HashMap<String, &'static str>) -> String {
    let col_name = quote_ident(&field.name);
    let is_pk = field.is_primary_key.unwrap_or(false) || field.name == "id";

    // For entity_link, resolve the PG type from the referenced entity's PK type
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

    // Primary key
    if is_pk {
        parts.push("PRIMARY KEY".to_string());
        if pg_type == "UUID" {
            parts.push("DEFAULT gen_random_uuid()".to_string());
        }
    }

    // NOT NULL for required fields (unless it's a PK which is always NOT NULL)
    if field.required && !is_pk {
        parts.push("NOT NULL".to_string());
    }

    // DEFAULT value
    if let Some(ref default_val) = field.default_value {
        if !is_pk {
            if let Some(default_sql) = json_to_sql_default(default_val, pg_type) {
                parts.push(format!("DEFAULT {default_sql}"));
            }
        }
    }

    let col_def = parts.join(" ");

    // CHECK constraint for enum values
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
        _ => "TEXT", // fallback
    }
}

/// Convert a serde_json Value to a SQL DEFAULT literal.
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

// ── SQL Safety ──────────────────────────────────────────────────────

/// Quote a SQL identifier to prevent injection.
/// Only allows alphanumeric and underscore characters.
fn quote_ident(ident: &str) -> String {
    let sanitized: String = ident
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    format!("\"{}\"", sanitized)
}
