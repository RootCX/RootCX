use std::collections::{HashMap, HashSet};

use sqlx::{Connection, PgPool};
use tracing::info;

use crate::RuntimeError;
use crate::manifest::{identity_index_name, json_to_sql_default, list_identity_indexes, map_field_type, quote_ident};
use rootcx_types::{EntityContract, FieldContract, SchemaChange, SchemaVerification};

const PROTECTED_COLUMNS: &[&str] = &["id", "created_at", "updated_at"];

#[derive(Debug, Clone, PartialEq)]
pub struct DbColumn {
    pub name: String,
    pub pg_type: String,
    pub not_null: bool,
    pub default_expr: Option<String>,
    pub check_constraints: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum ColumnDiff {
    Add { field: FieldContract, pg_type: String },
    Drop { column_name: String },
    AlterType { column_name: String, from_type: String, to_type: String },
    SetNotNull { column_name: String },
    DropNotNull { column_name: String },
    SetDefault { column_name: String, default_sql: String },
    DropDefault { column_name: String },
    ReplaceCheckConstraint { column_name: String, old_constraint_names: Vec<String>, new_values: Vec<String> },
    DropCheckConstraint { column_name: String, constraint_names: Vec<String> },
}

#[derive(Debug, Clone)]
pub struct TableDiff {
    pub schema_name: String,
    pub table_name: String,
    pub changes: Vec<ColumnDiff>,
}

fn normalize_pg_type(manifest_pg_type: &str) -> &str {
    match manifest_pg_type {
        "TEXT" => "text",
        "DOUBLE PRECISION" => "double precision",
        "BOOLEAN" => "boolean",
        "DATE" => "date",
        "TIMESTAMPTZ" => "timestamp with time zone",
        "JSONB" => "jsonb",
        "UUID" => "uuid",
        "TEXT[]" => "text[]",
        "DOUBLE PRECISION[]" => "double precision[]",
        other => other,
    }
}

fn resolve_pg_type(field: &FieldContract, pk_types: &HashMap<String, &'static str>) -> &'static str {
    if field.field_type == "entity_link" {
        if let Some(refs) = &field.references {
            return pk_types.get(&refs.entity).copied().unwrap_or("UUID");
        }
        return "UUID";
    }
    map_field_type(&field.field_type)
}

fn defaults_match(pg_expr: &str, desired: &str) -> bool {
    let normalized = pg_expr.split("::").next().unwrap_or(pg_expr).trim();
    normalized == desired.trim()
}

pub fn compute_diff(
    schema_name: &str,
    table_name: &str,
    db_columns: &[DbColumn],
    manifest_fields: &[FieldContract],
    pk_types: &HashMap<String, &'static str>,
) -> TableDiff {
    let mut changes = Vec::new();

    let db_map: HashMap<&str, &DbColumn> = db_columns.iter().map(|c| (c.name.as_str(), c)).collect();
    let manifest_names: HashSet<&str> = manifest_fields.iter().map(|f| f.name.as_str()).collect();

    for field in manifest_fields {
        if PROTECTED_COLUMNS.contains(&field.name.as_str()) {
            continue;
        }

        let desired_pg_type = resolve_pg_type(field, pk_types);
        let desired_normalized = normalize_pg_type(desired_pg_type);

        match db_map.get(field.name.as_str()) {
            None => {
                changes.push(ColumnDiff::Add { field: field.clone(), pg_type: desired_pg_type.to_string() });
            }
            Some(db_col) => {
                if db_col.pg_type != desired_normalized {
                    changes.push(ColumnDiff::AlterType {
                        column_name: field.name.clone(),
                        from_type: db_col.pg_type.clone(),
                        to_type: desired_pg_type.to_string(),
                    });
                }

                let is_pk = field.is_primary_key.unwrap_or(false) || field.name == "id";
                let want_not_null = field.required && !is_pk;
                if want_not_null && !db_col.not_null {
                    changes.push(ColumnDiff::SetNotNull { column_name: field.name.clone() });
                } else if !want_not_null && db_col.not_null && !is_pk {
                    changes.push(ColumnDiff::DropNotNull { column_name: field.name.clone() });
                }

                let desired_default =
                    field.default_value.as_ref().and_then(|v| json_to_sql_default(v, desired_pg_type));
                match (&db_col.default_expr, &desired_default) {
                    (None, Some(sql)) => {
                        changes
                            .push(ColumnDiff::SetDefault { column_name: field.name.clone(), default_sql: sql.clone() });
                    }
                    (Some(_), None) => {
                        changes.push(ColumnDiff::DropDefault { column_name: field.name.clone() });
                    }
                    (Some(existing), Some(desired)) => {
                        if !defaults_match(existing, desired) {
                            changes.push(ColumnDiff::SetDefault {
                                column_name: field.name.clone(),
                                default_sql: desired.clone(),
                            });
                        }
                    }
                    (None, None) => {}
                }

                let has_old_check = !db_col.check_constraints.is_empty();
                let has_new_enum = field.enum_values.as_ref().is_some_and(|v| !v.is_empty());
                match (has_old_check, has_new_enum) {
                    (true, true) => {
                        changes.push(ColumnDiff::ReplaceCheckConstraint {
                            column_name: field.name.clone(),
                            old_constraint_names: db_col.check_constraints.clone(),
                            new_values: field.enum_values.clone().unwrap(),
                        });
                    }
                    (true, false) => {
                        changes.push(ColumnDiff::DropCheckConstraint {
                            column_name: field.name.clone(),
                            constraint_names: db_col.check_constraints.clone(),
                        });
                    }
                    (false, true) => {
                        changes.push(ColumnDiff::ReplaceCheckConstraint {
                            column_name: field.name.clone(),
                            old_constraint_names: vec![],
                            new_values: field.enum_values.clone().unwrap(),
                        });
                    }
                    (false, false) => {}
                }
            }
        }
    }

    for db_col in db_columns {
        if PROTECTED_COLUMNS.contains(&db_col.name.as_str()) {
            continue;
        }
        if !manifest_names.contains(db_col.name.as_str()) {
            changes.push(ColumnDiff::Drop { column_name: db_col.name.clone() });
        }
    }

    TableDiff { schema_name: schema_name.to_string(), table_name: table_name.to_string(), changes }
}

fn generate_using_clause(col_name: &str, to: &str) -> String {
    let col = quote_ident(col_name);
    match to {
        "DOUBLE PRECISION" => format!(" USING NULLIF({col}, '')::DOUBLE PRECISION"),
        "BOOLEAN" => format!(
            " USING CASE WHEN {col} IN ('true','1','yes','t') THEN true \
             WHEN {col} IN ('false','0','no','f') THEN false ELSE NULL END"
        ),
        _ => format!(" USING {col}::{to}"),
    }
}

pub fn generate_ddl(diff: &TableDiff) -> Vec<String> {
    let fq = format!("{}.{}", quote_ident(&diff.schema_name), quote_ident(&diff.table_name));
    // Phase ordering: drop constraints → alter types → add columns → nullability → defaults → add constraints → drop columns
    let mut phases: [Vec<String>; 7] = Default::default();
    for change in &diff.changes {
        emit_ddl_phases(&fq, &diff.table_name, change, &mut phases);
    }
    phases.into_iter().flatten().collect()
}

fn emit_check_sql(fq: &str, table: &str, col_name: &str, values: &[String]) -> String {
    let col = quote_ident(col_name);
    let list: Vec<String> = values.iter().map(|v| format!("'{}'", v.replace('\'', "''"))).collect();
    format!(
        "ALTER TABLE {fq} ADD CONSTRAINT {} CHECK ({col} IN ({}))",
        quote_ident(&format!("chk_{table}_{col_name}")),
        list.join(", ")
    )
}

fn emit_ddl_phases(fq: &str, table: &str, change: &ColumnDiff, p: &mut [Vec<String>; 7]) {
    match change {
        ColumnDiff::Add { field, pg_type } => {
            let col = quote_ident(&field.name);
            p[2].push(format!("ALTER TABLE {fq} ADD COLUMN IF NOT EXISTS {col} {pg_type}"));
            if field.required {
                p[3].push(format!("ALTER TABLE {fq} ALTER COLUMN {col} SET NOT NULL"));
            }
            if let Some(ref val) = field.default_value
                && let Some(sql) = json_to_sql_default(val, pg_type) {
                    p[4].push(format!("ALTER TABLE {fq} ALTER COLUMN {col} SET DEFAULT {sql}"));
                }
            if let Some(ref vals) = field.enum_values
                && !vals.is_empty() {
                    p[5].push(emit_check_sql(fq, table, &field.name, vals));
                }
        }
        ColumnDiff::Drop { column_name } => {
            p[6].push(format!("ALTER TABLE {fq} DROP COLUMN IF EXISTS {} CASCADE", quote_ident(column_name)));
        }
        ColumnDiff::AlterType { column_name, to_type, .. } => {
            let col = quote_ident(column_name);
            let using = generate_using_clause(column_name, to_type);
            p[1].push(format!("ALTER TABLE {fq} ALTER COLUMN {col} TYPE {to_type}{using}"));
        }
        ColumnDiff::SetNotNull { column_name } => {
            p[3].push(format!("ALTER TABLE {fq} ALTER COLUMN {} SET NOT NULL", quote_ident(column_name)));
        }
        ColumnDiff::DropNotNull { column_name } => {
            p[3].push(format!("ALTER TABLE {fq} ALTER COLUMN {} DROP NOT NULL", quote_ident(column_name)));
        }
        ColumnDiff::SetDefault { column_name, default_sql } => {
            p[4].push(format!("ALTER TABLE {fq} ALTER COLUMN {} SET DEFAULT {default_sql}", quote_ident(column_name)));
        }
        ColumnDiff::DropDefault { column_name } => {
            p[4].push(format!("ALTER TABLE {fq} ALTER COLUMN {} DROP DEFAULT", quote_ident(column_name)));
        }
        ColumnDiff::ReplaceCheckConstraint { column_name, old_constraint_names, new_values } => {
            for name in old_constraint_names {
                p[0].push(format!("ALTER TABLE {fq} DROP CONSTRAINT IF EXISTS {}", quote_ident(name)));
            }
            p[5].push(emit_check_sql(fq, table, column_name, new_values));
        }
        ColumnDiff::DropCheckConstraint { constraint_names, .. } => {
            for name in constraint_names {
                p[0].push(format!("ALTER TABLE {fq} DROP CONSTRAINT IF EXISTS {}", quote_ident(name)));
            }
        }
    }
}

async fn apply_ddl(pool: &PgPool, statements: &[String]) -> Result<(), RuntimeError> {
    if statements.is_empty() {
        return Ok(());
    }
    let mut tx = pool.begin().await.map_err(RuntimeError::Schema)?;
    for stmt in statements {
        sqlx::query(stmt).execute(&mut *tx).await.map_err(RuntimeError::Schema)?;
    }
    tx.commit().await.map_err(RuntimeError::Schema)?;
    Ok(())
}

pub async fn sync_schema(
    pool: &PgPool,
    app_id: &str,
    entities: &[EntityContract],
    pk_types: &HashMap<String, &'static str>,
) -> Result<(), RuntimeError> {
    let mut has_type_changes = false;

    for entity in entities {
        let db_columns = introspect_table(pool, app_id, &entity.entity_name).await?;

        if db_columns.is_empty() {
            continue;
        }

        let diff = compute_diff(app_id, &entity.entity_name, &db_columns, &entity.fields, pk_types);

        if diff.changes.is_empty() {
            info!(
                table = %format!("{}.{}", app_id, entity.entity_name),
                "schema in sync"
            );
            continue;
        }

        has_type_changes |= diff.changes.iter().any(|c| matches!(c, ColumnDiff::AlterType { .. }));

        let stmts = generate_ddl(&diff);
        info!(
            table = %format!("{}.{}", app_id, entity.entity_name),
            changes = diff.changes.len(),
            statements = stmts.len(),
            "applying schema sync"
        );

        apply_ddl(pool, &stmts).await?;
    }

    // Invalidate sqlx prepared-statement caches after column type changes
    // to prevent stale parameter-type bindings (e.g. f64 sent as TEXT).
    if has_type_changes {
        let mut conns = Vec::new();
        while let Some(conn) = pool.try_acquire() {
            conns.push(conn);
        }
        for conn in &mut conns {
            conn.clear_cached_statements().await.ok();
        }
    }

    Ok(())
}

pub async fn introspect_table(
    pool: &PgPool,
    schema_name: &str,
    table_name: &str,
) -> Result<Vec<DbColumn>, RuntimeError> {
    let fq = format!("{}.{}", quote_ident(schema_name), quote_ident(table_name));
    let exists: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = $1 AND c.relname = $2 AND c.relkind = 'r'")
    .bind(schema_name)
    .bind(table_name)
    .fetch_optional(pool)
    .await
    .map_err(RuntimeError::Schema)?;

    if exists.is_none() {
        return Ok(vec![]);
    }

    let rows: Vec<(String, String, bool, Option<String>, Vec<String>)> = sqlx::query_as(&format!(
        "SELECT \
            a.attname, \
            format_type(a.atttypid, a.atttypmod), \
            a.attnotnull, \
            pg_get_expr(d.adbin, d.adrelid), \
            COALESCE(\
                (SELECT array_agg(con.conname) \
                 FROM pg_constraint con \
                 WHERE con.conrelid = a.attrelid \
                   AND con.contype = 'c' \
                   AND a.attnum = ANY(con.conkey)), \
                ARRAY[]::text[]\
            ) \
         FROM pg_attribute a \
         LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
         WHERE a.attrelid = '{fq}'::regclass \
           AND a.attnum > 0 \
           AND NOT a.attisdropped \
         ORDER BY a.attnum"
    ))
    .fetch_all(pool)
    .await
    .map_err(RuntimeError::Schema)?;

    Ok(rows
        .into_iter()
        .map(|(name, pg_type, not_null, default_expr, checks)| DbColumn {
            name,
            pg_type,
            not_null,
            default_expr,
            check_constraints: checks,
        })
        .collect())
}

pub async fn list_tables_in_schema(pool: &PgPool, schema_name: &str) -> Result<Vec<String>, RuntimeError> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT c.relname FROM pg_class c \
         JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = $1 AND c.relkind = 'r' \
         ORDER BY c.relname",
    )
    .bind(schema_name)
    .fetch_all(pool)
    .await
    .map_err(RuntimeError::Schema)?;

    Ok(rows.into_iter().map(|(name,)| name).collect())
}

pub(crate) fn find_orphaned_tables(db_tables: &[String], manifest_entities: &[EntityContract]) -> Vec<String> {
    let manifest_names: HashSet<&str> = manifest_entities.iter().map(|e| e.entity_name.as_str()).collect();
    db_tables.iter().filter(|t| !manifest_names.contains(t.as_str())).cloned().collect()
}

fn column_diff_to_schema_change(entity: &str, diff: &ColumnDiff) -> SchemaChange {
    let (change_type, column, detail) = match diff {
        ColumnDiff::Add { field, pg_type } => ("add_column", &field.name, Some(pg_type.clone())),
        ColumnDiff::Drop { column_name } => ("drop_column", column_name, None),
        ColumnDiff::AlterType { column_name, from_type, to_type } => {
            ("alter_type", column_name, Some(format!("{from_type} → {to_type}")))
        }
        ColumnDiff::SetNotNull { column_name } => ("set_not_null", column_name, None),
        ColumnDiff::DropNotNull { column_name } => ("drop_not_null", column_name, None),
        ColumnDiff::SetDefault { column_name, default_sql } => ("set_default", column_name, Some(default_sql.clone())),
        ColumnDiff::DropDefault { column_name } => ("drop_default", column_name, None),
        ColumnDiff::ReplaceCheckConstraint { column_name, new_values, .. } => {
            ("update_enum", column_name, Some(new_values.join(", ")))
        }
        ColumnDiff::DropCheckConstraint { column_name, .. } => ("drop_enum", column_name, None),
    };
    SchemaChange { entity: entity.to_string(), change_type: change_type.to_string(), column: column.clone(), detail }
}

pub async fn verify_all(
    pool: &PgPool,
    app_id: &str,
    entities: &[EntityContract],
    pk_types: &HashMap<String, &'static str>,
) -> Result<SchemaVerification, RuntimeError> {
    let mut changes = Vec::new();
    for entity in entities {
        let db_columns = introspect_table(pool, app_id, &entity.entity_name).await?;
        if db_columns.is_empty() {
            changes.push(SchemaChange {
                entity: entity.entity_name.clone(),
                change_type: "create_table".to_string(),
                column: String::new(),
                detail: Some(format!("{} fields", entity.fields.len())),
            });
            continue;
        }
        let diff = compute_diff(app_id, &entity.entity_name, &db_columns, &entity.fields, pk_types);
        changes.extend(diff.changes.iter().map(|c| column_diff_to_schema_change(&entity.entity_name, c)));
    }

    let db_tables = list_tables_in_schema(pool, app_id).await?;
    for table in find_orphaned_tables(&db_tables, entities) {
        changes.push(SchemaChange {
            entity: table,
            change_type: "drop_table".to_string(),
            column: String::new(),
            detail: None,
        });
    }

    changes.extend(verify_identity_indexes(pool, app_id, entities).await?);

    Ok(SchemaVerification { compliant: changes.is_empty(), changes })
}

async fn verify_identity_indexes(
    pool: &PgPool,
    app_id: &str,
    entities: &[EntityContract],
) -> Result<Vec<SchemaChange>, RuntimeError> {
    let mut changes = Vec::new();
    for entity in entities {
        let expected = identity_index_name(entity);
        let existing = list_identity_indexes(pool, app_id, &entity.entity_name).await?;

        if let Some(ref idx_name) = expected {
            if !existing.contains(idx_name) {
                changes.push(SchemaChange {
                    entity: entity.entity_name.clone(),
                    change_type: "add_identity_index".to_string(),
                    column: entity.identity_key.clone().unwrap_or_default(),
                    detail: entity.identity_kind.clone(),
                });
            }
        }

        for name in &existing {
            if expected.as_ref() != Some(name) {
                changes.push(SchemaChange {
                    entity: entity.entity_name.clone(),
                    change_type: "drop_identity_index".to_string(),
                    column: name.clone(),
                    detail: None,
                });
            }
        }
    }
    Ok(changes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rootcx_types::{FieldContract, FieldReference};
    use serde_json::json;

    fn db_col(name: &str, pg_type: &str, not_null: bool) -> DbColumn {
        DbColumn {
            name: name.to_string(),
            pg_type: pg_type.to_string(),
            not_null,
            default_expr: None,
            check_constraints: vec![],
        }
    }

    fn db_col_with_default(name: &str, pg_type: &str, not_null: bool, default: &str) -> DbColumn {
        DbColumn {
            name: name.to_string(),
            pg_type: pg_type.to_string(),
            not_null,
            default_expr: Some(default.to_string()),
            check_constraints: vec![],
        }
    }

    fn db_col_with_check(name: &str, pg_type: &str, not_null: bool, checks: Vec<&str>) -> DbColumn {
        DbColumn {
            name: name.to_string(),
            pg_type: pg_type.to_string(),
            not_null,
            default_expr: None,
            check_constraints: checks.into_iter().map(String::from).collect(),
        }
    }

    fn mfield(name: &str, field_type: &str) -> FieldContract {
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

    fn empty_pk_types() -> HashMap<String, &'static str> {
        HashMap::new()
    }

    fn find_diff<'a>(changes: &'a [ColumnDiff], pred: impl Fn(&ColumnDiff) -> bool) -> &'a ColumnDiff {
        changes.iter().find(|c| pred(c)).expect("expected diff not found")
    }

    fn mentity(name: &str, fields: Vec<FieldContract>) -> EntityContract {
        EntityContract { entity_name: name.to_string(), fields, identity_kind: None, identity_key: None }
    }

    // ── normalize / defaults ─────────────────────────────────────────

    #[test]
    fn normalize_all_types() {
        for (input, expected) in [
            ("TEXT", "text"),
            ("DOUBLE PRECISION", "double precision"),
            ("BOOLEAN", "boolean"),
            ("DATE", "date"),
            ("TIMESTAMPTZ", "timestamp with time zone"),
            ("JSONB", "jsonb"),
            ("UUID", "uuid"),
            ("TEXT[]", "text[]"),
            ("DOUBLE PRECISION[]", "double precision[]"),
        ] {
            assert_eq!(normalize_pg_type(input), expected, "normalize({input})");
        }
    }

    #[test]
    fn defaults_match_cases() {
        assert!(defaults_match("'hello'", "'hello'"));
        assert!(defaults_match("'hello'::text", "'hello'"));
        assert!(defaults_match("42", "42"));
        assert!(defaults_match("42::double precision", "42"));
        assert!(!defaults_match("'old'", "'new'"));
        assert!(!defaults_match("'old'::text", "'new'"));
    }

    // ── compute_diff ─────────────────────────────────────────────────

    #[test]
    fn diff_no_changes() {
        let db = vec![db_col("name", "text", false)];
        let manifest = vec![mfield("name", "text")];
        let diff = compute_diff("app", "tbl", &db, &manifest, &empty_pk_types());
        assert!(diff.changes.is_empty(), "expected no changes: {:?}", diff.changes);
    }

    #[test]
    fn diff_add_column() {
        let db = vec![db_col("name", "text", false)];
        let manifest = vec![mfield("name", "text"), mfield("email", "text")];
        let diff = compute_diff("app", "tbl", &db, &manifest, &empty_pk_types());
        assert_eq!(diff.changes.len(), 1);
        match &diff.changes[0] {
            ColumnDiff::Add { field, pg_type } => {
                assert_eq!(field.name, "email");
                assert_eq!(pg_type, "TEXT");
            }
            other => panic!("expected Add, got: {other:?}"),
        }
    }

    #[test]
    fn diff_add_multiple_columns() {
        let db = vec![db_col("name", "text", false)];
        let manifest = vec![mfield("name", "text"), mfield("email", "text"), mfield("age", "number")];
        let diff = compute_diff("app", "tbl", &db, &manifest, &empty_pk_types());
        let adds: Vec<_> = diff.changes.iter().filter(|c| matches!(c, ColumnDiff::Add { .. })).collect();
        assert_eq!(adds.len(), 2);
    }

    #[test]
    fn diff_drop_column() {
        let db = vec![db_col("name", "text", false), db_col("legacy", "text", false)];
        let manifest = vec![mfield("name", "text")];
        let diff = compute_diff("app", "tbl", &db, &manifest, &empty_pk_types());
        assert_eq!(diff.changes.len(), 1);
        match &diff.changes[0] {
            ColumnDiff::Drop { column_name } => assert_eq!(column_name, "legacy"),
            other => panic!("expected Drop, got: {other:?}"),
        }
    }

    #[test]
    fn diff_protected_columns_never_dropped() {
        let db = vec![
            db_col("id", "uuid", true),
            db_col("created_at", "timestamp with time zone", true),
            db_col("updated_at", "timestamp with time zone", true),
            db_col("name", "text", false),
        ];
        let manifest = vec![mfield("name", "text")];
        let diff = compute_diff("app", "tbl", &db, &manifest, &empty_pk_types());
        let drops: Vec<_> = diff.changes.iter().filter(|c| matches!(c, ColumnDiff::Drop { .. })).collect();
        assert!(drops.is_empty(), "protected columns should never be dropped: {drops:?}");
    }

    #[test]
    fn diff_alter_type_text_to_number() {
        let db = vec![db_col("score", "text", false)];
        let manifest = vec![mfield("score", "number")];
        let diff = compute_diff("app", "tbl", &db, &manifest, &empty_pk_types());
        let alter = find_diff(&diff.changes, |c| matches!(c, ColumnDiff::AlterType { .. }));
        match alter {
            ColumnDiff::AlterType { from_type, to_type, .. } => {
                assert_eq!(from_type, "text");
                assert_eq!(to_type, "DOUBLE PRECISION");
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn diff_alter_type_number_to_text() {
        let db = vec![db_col("score", "double precision", false)];
        let manifest = vec![mfield("score", "text")];
        let diff = compute_diff("app", "tbl", &db, &manifest, &empty_pk_types());
        let alter = find_diff(&diff.changes, |c| matches!(c, ColumnDiff::AlterType { .. }));
        match alter {
            ColumnDiff::AlterType { from_type, to_type, .. } => {
                assert_eq!(from_type, "double precision");
                assert_eq!(to_type, "TEXT");
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn diff_set_not_null() {
        let db = vec![db_col("email", "text", false)];
        let mut f = mfield("email", "text");
        f.required = true;
        let diff = compute_diff("app", "tbl", &db, &[f], &empty_pk_types());
        assert!(
            diff.changes.iter().any(|c| matches!(c, ColumnDiff::SetNotNull { column_name } if column_name == "email"))
        );
    }

    #[test]
    fn diff_drop_not_null() {
        let db = vec![db_col("email", "text", true)];
        let manifest = vec![mfield("email", "text")]; // required = false
        let diff = compute_diff("app", "tbl", &db, &manifest, &empty_pk_types());
        assert!(
            diff.changes.iter().any(|c| matches!(c, ColumnDiff::DropNotNull { column_name } if column_name == "email"))
        );
    }

    #[test]
    fn diff_add_default() {
        let db = vec![db_col("status", "text", false)];
        let mut f = mfield("status", "text");
        f.default_value = Some(json!("N/A"));
        let diff = compute_diff("app", "tbl", &db, &[f], &empty_pk_types());
        assert!(diff.changes.iter().any(|c| matches!(c, ColumnDiff::SetDefault { column_name, default_sql }
                if column_name == "status" && default_sql == "'N/A'")));
    }

    #[test]
    fn diff_drop_default() {
        let db = vec![db_col_with_default("status", "text", false, "'old'::text")];
        let manifest = vec![mfield("status", "text")]; // no default
        let diff = compute_diff("app", "tbl", &db, &manifest, &empty_pk_types());
        assert!(
            diff.changes
                .iter()
                .any(|c| matches!(c, ColumnDiff::DropDefault { column_name } if column_name == "status"))
        );
    }

    #[test]
    fn diff_add_enum_check() {
        let db = vec![db_col("color", "text", false)];
        let mut f = mfield("color", "text");
        f.enum_values = Some(vec!["red".into(), "blue".into()]);
        let diff = compute_diff("app", "tbl", &db, &[f], &empty_pk_types());
        let check = find_diff(&diff.changes, |c| matches!(c, ColumnDiff::ReplaceCheckConstraint { .. }));
        match check {
            ColumnDiff::ReplaceCheckConstraint { old_constraint_names, new_values, .. } => {
                assert!(old_constraint_names.is_empty());
                assert_eq!(new_values, &["red", "blue"]);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn diff_change_enum_check() {
        let db = vec![db_col_with_check("color", "text", false, vec!["chk_tbl_color"])];
        let mut f = mfield("color", "text");
        f.enum_values = Some(vec!["red".into(), "green".into()]);
        let diff = compute_diff("app", "tbl", &db, &[f], &empty_pk_types());
        let check = find_diff(&diff.changes, |c| matches!(c, ColumnDiff::ReplaceCheckConstraint { .. }));
        match check {
            ColumnDiff::ReplaceCheckConstraint { old_constraint_names, new_values, .. } => {
                assert_eq!(old_constraint_names, &["chk_tbl_color"]);
                assert_eq!(new_values, &["red", "green"]);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn diff_drop_enum_check() {
        let db = vec![db_col_with_check("color", "text", false, vec!["chk_tbl_color"])];
        let manifest = vec![mfield("color", "text")]; // no enum
        let diff = compute_diff("app", "tbl", &db, &manifest, &empty_pk_types());
        let check = find_diff(&diff.changes, |c| matches!(c, ColumnDiff::DropCheckConstraint { .. }));
        match check {
            ColumnDiff::DropCheckConstraint { constraint_names, .. } => {
                assert_eq!(constraint_names, &["chk_tbl_color"]);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn diff_combined_changes() {
        // Column exists as "text, nullable, no default" but manifest wants "number, required, default 0"
        let db = vec![db_col("score", "text", false)];
        let mut f = mfield("score", "number");
        f.required = true;
        f.default_value = Some(json!(0));
        let diff = compute_diff("app", "tbl", &db, &[f], &empty_pk_types());
        assert!(diff.changes.iter().any(|c| matches!(c, ColumnDiff::AlterType { .. })));
        assert!(diff.changes.iter().any(|c| matches!(c, ColumnDiff::SetNotNull { .. })));
        assert!(diff.changes.iter().any(|c| matches!(c, ColumnDiff::SetDefault { .. })));
    }

    #[test]
    fn diff_entity_link_resolution() {
        let mut pk = HashMap::new();
        pk.insert("accounts".to_string(), "TEXT" as &str);
        let db = vec![]; // no columns = new column
        let mut f = mfield("account_id", "entity_link");
        f.references = Some(FieldReference { entity: "accounts".to_string(), field: "id".to_string() });
        let diff = compute_diff("app", "tbl", &db, &[f], &pk);
        match &diff.changes[0] {
            ColumnDiff::Add { pg_type, .. } => assert_eq!(pg_type, "TEXT"),
            other => panic!("expected Add, got: {other:?}"),
        }
    }

    #[test]
    fn compute_diff_empty_db_produces_adds_for_all_fields() {
        let fields = vec![mfield("name", "text"), mfield("email", "text")];
        let diff = compute_diff("app", "tbl", &[], &fields, &empty_pk_types());
        assert_eq!(diff.changes.len(), 2);
        assert!(
            diff.changes.iter().all(|c| matches!(c, ColumnDiff::Add { .. })),
            "all changes should be Add: {:?}",
            diff.changes
        );
    }

    // ── generate_ddl ─────────────────────────────────────────────────

    #[test]
    fn ddl_empty_diff() {
        let diff = TableDiff { schema_name: "app".into(), table_name: "tbl".into(), changes: vec![] };
        assert!(generate_ddl(&diff).is_empty());
    }

    #[test]
    fn ddl_add_column_format() {
        let diff = TableDiff {
            schema_name: "app".into(),
            table_name: "tbl".into(),
            changes: vec![ColumnDiff::Add { field: mfield("email", "text"), pg_type: "TEXT".into() }],
        };
        let stmts = generate_ddl(&diff);
        assert!(
            stmts.iter().any(|s| s.contains("ADD COLUMN IF NOT EXISTS") && s.contains("\"email\"") && s.contains("TEXT")),
            "stmts: {stmts:?}"
        );
    }

    #[test]
    fn ddl_drop_column_cascade() {
        let diff = TableDiff {
            schema_name: "app".into(),
            table_name: "tbl".into(),
            changes: vec![ColumnDiff::Drop { column_name: "legacy".into() }],
        };
        let stmts = generate_ddl(&diff);
        assert!(
            stmts.iter().any(|s| s.contains("DROP COLUMN IF EXISTS") && s.contains("\"legacy\"") && s.contains("CASCADE")),
            "stmts: {stmts:?}"
        );
    }

    #[test]
    fn ddl_alter_type_using() {
        let diff = TableDiff {
            schema_name: "app".into(),
            table_name: "tbl".into(),
            changes: vec![ColumnDiff::AlterType {
                column_name: "score".into(),
                from_type: "text".into(),
                to_type: "DOUBLE PRECISION".into(),
            }],
        };
        let stmts = generate_ddl(&diff);
        let stmt = &stmts[0];
        assert!(stmt.contains("ALTER COLUMN"), "stmt: {stmt}");
        assert!(stmt.contains("TYPE DOUBLE PRECISION"), "stmt: {stmt}");
        assert!(stmt.contains("USING"), "stmt: {stmt}");
    }

    #[test]
    fn ddl_statement_ordering() {
        let diff = TableDiff {
            schema_name: "app".into(),
            table_name: "tbl".into(),
            changes: vec![
                ColumnDiff::ReplaceCheckConstraint {
                    column_name: "status".into(),
                    old_constraint_names: vec!["old_chk".into()],
                    new_values: vec!["a".into(), "b".into()],
                },
                ColumnDiff::AlterType {
                    column_name: "score".into(),
                    from_type: "text".into(),
                    to_type: "DOUBLE PRECISION".into(),
                },
                ColumnDiff::Add { field: mfield("email", "text"), pg_type: "TEXT".into() },
                ColumnDiff::SetNotNull { column_name: "name".into() },
                ColumnDiff::SetDefault { column_name: "role".into(), default_sql: "'user'".into() },
                ColumnDiff::Drop { column_name: "legacy".into() },
            ],
        };
        let stmts = generate_ddl(&diff);

        let drop_check_pos = stmts.iter().position(|s| s.contains("DROP CONSTRAINT")).unwrap();
        let alter_type_pos = stmts.iter().position(|s| s.contains("TYPE DOUBLE PRECISION")).unwrap();
        let add_col_pos = stmts.iter().position(|s| s.contains("ADD COLUMN")).unwrap();
        let set_not_null_pos = stmts.iter().position(|s| s.contains("SET NOT NULL")).unwrap();
        let set_default_pos = stmts.iter().position(|s| s.contains("SET DEFAULT")).unwrap();
        let add_check_pos = stmts.iter().position(|s| s.contains("ADD CONSTRAINT")).unwrap();
        let drop_col_pos = stmts.iter().position(|s| s.contains("DROP COLUMN")).unwrap();

        assert!(drop_check_pos < alter_type_pos, "DROP CHECK before ALTER TYPE");
        assert!(alter_type_pos < add_col_pos, "ALTER TYPE before ADD COLUMN");
        assert!(add_col_pos < set_not_null_pos, "ADD COLUMN before SET NOT NULL");
        assert!(set_not_null_pos < set_default_pos, "SET NOT NULL before SET DEFAULT");
        assert!(set_default_pos < add_check_pos, "SET DEFAULT before ADD CHECK");
        assert!(add_check_pos < drop_col_pos, "ADD CHECK before DROP COLUMN");
    }

    #[test]
    fn ddl_check_replacement() {
        let diff = TableDiff {
            schema_name: "app".into(),
            table_name: "tbl".into(),
            changes: vec![ColumnDiff::ReplaceCheckConstraint {
                column_name: "status".into(),
                old_constraint_names: vec!["old_chk".into()],
                new_values: vec!["draft".into(), "active".into()],
            }],
        };
        let stmts = generate_ddl(&diff);
        assert!(
            stmts.iter().any(|s| s.contains("DROP CONSTRAINT IF EXISTS") && s.contains("\"old_chk\"")),
            "should drop old constraint: {stmts:?}"
        );
        assert!(
            stmts.iter().any(|s| s.contains("ADD CONSTRAINT") && s.contains("'draft'") && s.contains("'active'")),
            "should add new constraint: {stmts:?}"
        );
    }

    // ── column_diff_to_schema_change ─────────────────────────────────

    #[test]
    fn column_diff_to_schema_change_all_variants() {
        let cases: Vec<(ColumnDiff, &str, &str, Option<&str>)> = vec![
            (
                ColumnDiff::Add { field: mfield("email", "text"), pg_type: "TEXT".into() },
                "add_column",
                "email",
                Some("TEXT"),
            ),
            (ColumnDiff::Drop { column_name: "legacy".into() }, "drop_column", "legacy", None),
            (
                ColumnDiff::AlterType {
                    column_name: "score".into(),
                    from_type: "text".into(),
                    to_type: "DOUBLE PRECISION".into(),
                },
                "alter_type",
                "score",
                Some("text → DOUBLE PRECISION"),
            ),
            (ColumnDiff::SetNotNull { column_name: "email".into() }, "set_not_null", "email", None),
            (ColumnDiff::DropNotNull { column_name: "email".into() }, "drop_not_null", "email", None),
            (
                ColumnDiff::SetDefault { column_name: "status".into(), default_sql: "'active'".into() },
                "set_default",
                "status",
                Some("'active'"),
            ),
            (ColumnDiff::DropDefault { column_name: "status".into() }, "drop_default", "status", None),
            (
                ColumnDiff::ReplaceCheckConstraint {
                    column_name: "color".into(),
                    old_constraint_names: vec![],
                    new_values: vec!["red".into(), "blue".into()],
                },
                "update_enum",
                "color",
                Some("red, blue"),
            ),
            (
                ColumnDiff::DropCheckConstraint { column_name: "color".into(), constraint_names: vec!["chk".into()] },
                "drop_enum",
                "color",
                None,
            ),
        ];

        for (diff, expected_type, expected_col, expected_detail) in cases {
            let sc = column_diff_to_schema_change("tbl", &diff);
            assert_eq!(sc.entity, "tbl");
            assert_eq!(sc.change_type, expected_type, "diff: {diff:?}");
            assert_eq!(sc.column, expected_col, "diff: {diff:?}");
            assert_eq!(sc.detail.as_deref(), expected_detail, "diff: {diff:?}");
        }
    }

    // ── orphaned tables ──────────────────────────────────────────────

    #[test]
    fn find_orphaned_tables_detects_removed_entity() {
        let db_tables = vec!["tasks".to_string(), "notes".to_string()];
        let entities = vec![mentity("tasks", vec![mfield("name", "text")])];
        let orphans = find_orphaned_tables(&db_tables, &entities);
        assert_eq!(orphans, vec!["notes"]);
    }

    #[test]
    fn find_orphaned_tables_no_orphans() {
        let db_tables = vec!["tasks".to_string()];
        let entities = vec![mentity("tasks", vec![mfield("name", "text")])];
        let orphans = find_orphaned_tables(&db_tables, &entities);
        assert!(orphans.is_empty());
    }

    #[test]
    fn find_orphaned_tables_empty_manifest() {
        let db_tables = vec!["tasks".to_string(), "notes".to_string()];
        let entities: Vec<EntityContract> = vec![];
        let orphans = find_orphaned_tables(&db_tables, &entities);
        assert_eq!(orphans, vec!["tasks", "notes"]);
    }

    #[test]
    fn find_orphaned_tables_empty_db() {
        let db_tables: Vec<String> = vec![];
        let entities = vec![mentity("tasks", vec![mfield("name", "text")])];
        let orphans = find_orphaned_tables(&db_tables, &entities);
        assert!(orphans.is_empty());
    }

}
