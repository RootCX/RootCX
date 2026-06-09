use std::collections::{HashMap, HashSet};

use sqlx::{Connection, PgPool};
use tracing::info;

use crate::RuntimeError;
use crate::manifest::{identity_index_name, is_system_field, json_to_sql_default, list_identity_indexes, map_field_type, quote_ident, resolve_on_delete};
use rootcx_types::{EntityContract, FieldContract, SchemaChange, SchemaVerification};

#[derive(Debug, Clone, PartialEq)]
pub struct FkInfo {
    pub constraint_name: String,
    pub delete_rule: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DbColumn {
    pub name: String,
    pub pg_type: String,
    pub not_null: bool,
    pub default_expr: Option<String>,
    pub fk: Option<FkInfo>,
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
    // CHECK constraints (enum-derived AND declarative) are NOT column-diffs — they
    // are reconciled by tag in `reconcile_checks`, exactly like indexes.
    ReplaceFkConstraint { column_name: String, old_constraint_name: String, new_constraint_name: String, target_table: String, delete_rule: String },
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

fn pg_delete_rule_char_to_str(c: &str) -> &str {
    match c {
        "c" => "CASCADE",
        "r" | "a" => "RESTRICT",
        "n" => "SET NULL",
        _ => "RESTRICT",
    }
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
        if is_system_field(&field.name) {
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

                // Enum CHECKs are reconciled by `reconcile_checks` (tag-owned),
                // not here — see the declarative-checks section below.

                if field.field_type == "entity_link" {
                    if let (Some(fk_info), Some(refs)) = (&db_col.fk, &field.references) {
                        let desired = resolve_on_delete(field);
                        let current = pg_delete_rule_char_to_str(&fk_info.delete_rule);
                        if current != desired {
                            let (target_schema, target_entity) = match crate::manifest::parse_entity_ref(&refs.entity) {
                                crate::manifest::RefTarget::Core(name) => {
                                    match crate::manifest::resolve_core_entity(&name) {
                                        Some((s, t, _, _)) => (s.to_string(), t.to_string()),
                                        None => continue,
                                    }
                                }
                                _ => (schema_name.to_string(), refs.entity.clone()),
                            };
                            changes.push(ColumnDiff::ReplaceFkConstraint {
                                column_name: field.name.clone(),
                                old_constraint_name: fk_info.constraint_name.clone(),
                                new_constraint_name: format!("fk_{}_{}_{}_{}",
                                    schema_name, table_name, field.name, target_entity),
                                target_table: format!("{}.{}", quote_ident(&target_schema), quote_ident(&target_entity)),
                                delete_rule: desired.to_string(),
                            });
                        }
                    }
                }
            }
        }
    }

    for db_col in db_columns {
        if is_system_field(&db_col.name) {
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
        emit_ddl_phases(&fq, change, &mut phases);
    }
    phases.into_iter().flatten().collect()
}

fn emit_ddl_phases(fq: &str, change: &ColumnDiff, p: &mut [Vec<String>; 7]) {
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
        ColumnDiff::ReplaceFkConstraint { column_name, old_constraint_name, new_constraint_name, target_table, delete_rule } => {
            p[0].push(format!("ALTER TABLE {fq} DROP CONSTRAINT IF EXISTS {}", quote_ident(old_constraint_name)));
            p[5].push(format!(
                "ALTER TABLE {fq} ADD CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {}({}) ON DELETE {delete_rule}",
                quote_ident(new_constraint_name), quote_ident(column_name), target_table, quote_ident("id")
            ));
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

        if !diff.changes.is_empty() {
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

        // Reconcile indexes + CHECKs in the same pass — columns are already
        // applied above, so anything they reference exists. No re-introspection.
        reconcile_indexes(pool, app_id, entity).await?;
        reconcile_checks(pool, app_id, entity).await?;
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

// ── Declarative secondary indexes ────────────────────────────────────
//
// Reconciled against pg_indexes. We only ever touch indexes WE own, tagged with
// a `rootcx:idx:<hash>` comment carrying a hash of the declared spec — so the
// PK, identity, and FK indexes are never touched, AND a changed definition under
// the same name is detected (hash differs) and replaced. Add/keep/change/remove
// are decided by the pure `compute_managed_diff`, then applied.

const MANAGED_INDEX_PREFIX: &str = "rootcx:idx:";

/// Stable hash of an index's DEFINITION (not its name): columns + per-column
/// sort/nulls/ops/expr, unique, method, partial predicate, storage params.
/// Same name + different definition → different hash → replace.
fn index_spec_hash(idx: &rootcx_types::IndexContract) -> String {
    use rootcx_types::IndexColumn;
    let mut s = String::new();
    s.push_str(if idx.unique { "u|" } else { "_|" });
    s.push_str(idx.using.as_deref().unwrap_or("btree"));
    s.push('|');
    for c in &idx.columns {
        match c {
            IndexColumn::Name(n) => s.push_str(n),
            IndexColumn::Spec(sp) => {
                for f in [&sp.column, &sp.expr, &sp.sort, &sp.nulls, &sp.ops] {
                    s.push_str(f.as_deref().unwrap_or(""));
                    s.push(':');
                }
            }
        }
        s.push(',');
    }
    s.push('|');
    s.push_str(idx.where_clause.as_deref().unwrap_or(""));
    s.push('|');
    for (k, v) in &idx.with {
        s.push_str(k);
        s.push('=');
        s.push_str(v);
        s.push(';');
    }
    fnv1a_hex(&s)
}

/// FNV-1a hex digest — the shared spec-hash primitive for indexes and checks.
fn fnv1a_hex(s: &str) -> String {
    let mut h = 0xcbf29ce484222325u64;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

#[derive(Debug, PartialEq, Eq)]
pub enum ManagedOp {
    /// Declared, not currently managed — create it.
    Create(String),
    /// Declared and managed, but the spec changed — drop + recreate.
    Replace(String),
    /// Present with the exact declared spec — no churn.
    Keep(String),
    /// Managed but no longer declared — drop it.
    Drop(String),
}

impl ManagedOp {
    pub fn name(&self) -> &str {
        match self {
            ManagedOp::Create(n) | ManagedOp::Replace(n) | ManagedOp::Keep(n) | ManagedOp::Drop(n) => n,
        }
    }
}

/// Pure add/keep/change/remove decision. `desired` = (name, spec-hash) for each
/// declared index; `managed` = name → stored spec-hash for indexes we own.
pub fn compute_managed_diff(
    desired: &[(String, String)],
    managed: &HashMap<String, String>,
) -> Vec<ManagedOp> {
    let mut ops = Vec::new();
    let desired_names: HashSet<&str> = desired.iter().map(|(n, _)| n.as_str()).collect();
    for (name, hash) in desired {
        match managed.get(name) {
            None => ops.push(ManagedOp::Create(name.clone())),
            Some(h) if h == hash => ops.push(ManagedOp::Keep(name.clone())),
            Some(_) => ops.push(ManagedOp::Replace(name.clone())),
        }
    }
    for name in managed.keys() {
        if !desired_names.contains(name.as_str()) {
            ops.push(ManagedOp::Drop(name.clone()));
        }
    }
    ops
}

fn index_method(using: Option<&str>) -> Result<&str, String> {
    match using.unwrap_or("btree").to_lowercase().as_str() {
        "btree" => Ok("btree"),
        "hash" => Ok("hash"),
        "gist" => Ok("gist"),
        "gin" => Ok("gin"),
        "spgist" => Ok("spgist"),
        "brin" => Ok("brin"),
        other => Err(format!("unknown index method '{other}'")),
    }
}

/// Render one index element: `col|（expr)` [opclass] [ASC|DESC] [NULLS …].
fn render_index_element(col: &rootcx_types::IndexColumn) -> Result<String, String> {
    use rootcx_types::IndexColumn;
    let (base, sort, nulls, ops) = match col {
        IndexColumn::Name(n) => (quote_ident(n), None, None, None),
        IndexColumn::Spec(s) => {
            let base = match (&s.column, &s.expr) {
                (Some(c), None) => quote_ident(c),
                (None, Some(e)) => format!("({e})"),
                (Some(_), Some(_)) => return Err("index column has both 'column' and 'expr'".into()),
                (None, None) => return Err("index column has neither 'column' nor 'expr'".into()),
            };
            (base, s.sort.as_deref(), s.nulls.as_deref(), s.ops.as_deref())
        }
    };
    let mut out = base;
    if let Some(o) = ops {
        out.push(' ');
        out.push_str(o); // operator class — a bare identifier fragment
    }
    if let Some(s) = sort {
        out.push_str(match s.to_lowercase().as_str() {
            "asc" => " ASC",
            "desc" => " DESC",
            other => return Err(format!("invalid sort '{other}' (asc|desc)")),
        });
    }
    if let Some(n) = nulls {
        out.push_str(match n.to_lowercase().as_str() {
            "first" => " NULLS FIRST",
            "last" => " NULLS LAST",
            other => return Err(format!("invalid nulls '{other}' (first|last)")),
        });
    }
    Ok(out)
}

/// Deterministic index name when the manifest omits one: `ix_<entity>_<n>` with
/// a short hash of the full spec so two unnamed indexes never collide. Capped at
/// Postgres's 63-byte identifier limit.
fn resolve_index_name(entity: &str, idx: &rootcx_types::IndexContract) -> String {
    if let Some(n) = &idx.name {
        return n.clone();
    }
    use rootcx_types::IndexColumn;
    let parts: Vec<String> = idx.columns.iter().map(|c| match c {
        IndexColumn::Name(n) => n.clone(),
        IndexColumn::Spec(s) => s.column.clone().unwrap_or_else(|| "expr".into()),
    }).collect();
    let mut hash = 0u64;
    for b in format!("{:?}{}{:?}", parts, idx.unique, idx.where_clause).bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(b as u64);
    }
    let base = format!("ix_{entity}_{}{}", parts.join("_"), if idx.unique { "_uq" } else { "" });
    let suffix = format!("_{:x}", hash & 0xffffff);
    let max = 63 - suffix.len();
    format!("{}{}", base.chars().take(max).collect::<String>(), suffix)
}

/// Build the `CREATE INDEX` statement for one declared index.
pub fn generate_create_index(
    schema: &str,
    table: &str,
    idx: &rootcx_types::IndexContract,
) -> Result<String, String> {
    let name = resolve_index_name(table, idx);
    let method = index_method(idx.using.as_deref())?;
    if idx.columns.is_empty() {
        return Err(format!("index '{name}' has no columns"));
    }
    let elems: Vec<String> = idx.columns.iter().map(render_index_element).collect::<Result<_, _>>()?;
    let fq = format!("{}.{}", quote_ident(schema), quote_ident(table));
    let mut sql = format!(
        "CREATE {}INDEX {} ON {fq} USING {method} ({})",
        if idx.unique { "UNIQUE " } else { "" },
        quote_ident(&name),
        elems.join(", "),
    );
    if !idx.with.is_empty() {
        let params: Vec<String> = idx.with.iter().map(|(k, v)| format!("{k} = {v}")).collect();
        sql.push_str(&format!(" WITH ({})", params.join(", ")));
    }
    if let Some(w) = &idx.where_clause {
        sql.push_str(&format!(" WHERE {w}"));
    }
    Ok(sql)
}

/// Resolved name → declared spec, rejecting duplicate names. Shared by reconcile
/// and verify so both see the same desired set.
fn desired_indexes(
    entity: &EntityContract,
) -> Result<HashMap<String, &rootcx_types::IndexContract>, String> {
    let mut by_name = HashMap::new();
    for i in &entity.indexes {
        let name = resolve_index_name(&entity.entity_name, i);
        if by_name.insert(name.clone(), i).is_some() {
            return Err(format!("duplicate index name '{name}' on '{}'", entity.entity_name));
        }
    }
    Ok(by_name)
}

/// name → stored spec-hash for the indexes WE manage, read from our
/// `rootcx:idx:<hash>` comments. PK / identity / FK indexes carry no such
/// comment, so they never appear here — and are thus never reconciled away.
async fn read_managed_index_hashes(
    pool: &PgPool,
    schema: &str,
    table: &str,
) -> Result<HashMap<String, String>, RuntimeError> {
    let rows: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT c.relname, obj_description(c.oid, 'pg_class') \
         FROM pg_index x \
         JOIN pg_class c ON c.oid = x.indexrelid \
         JOIN pg_class t ON t.oid = x.indrelid \
         JOIN pg_namespace n ON n.oid = t.relnamespace \
         WHERE n.nspname = $1 AND t.relname = $2 \
           AND obj_description(c.oid, 'pg_class') LIKE $3",
    )
    .bind(schema).bind(table).bind(format!("{MANAGED_INDEX_PREFIX}%"))
    .fetch_all(pool).await.map_err(RuntimeError::Schema)?;
    Ok(rows
        .into_iter()
        .filter_map(|(name, comment)| {
            comment.and_then(|c| c.strip_prefix(MANAGED_INDEX_PREFIX).map(|h| (name, h.to_string())))
        })
        .collect())
}

/// Reconcile declared indexes for one entity: add new, replace changed, drop
/// removed. Idempotent (unchanged indexes are kept untouched).
async fn reconcile_indexes(
    pool: &PgPool,
    schema: &str,
    entity: &EntityContract,
) -> Result<(), RuntimeError> {
    let proto = |m: String| RuntimeError::Schema(sqlx::Error::Protocol(m));
    let by_name = desired_indexes(entity).map_err(proto)?;
    let desired: Vec<(String, String)> =
        by_name.iter().map(|(n, i)| (n.clone(), index_spec_hash(i))).collect();
    let managed = read_managed_index_hashes(pool, schema, &entity.entity_name).await?;

    let ops = compute_managed_diff(&desired, &managed);
    if ops.iter().all(|o| matches!(o, ManagedOp::Keep(_))) {
        return Ok(());
    }

    let mut tx = pool.begin().await.map_err(RuntimeError::Schema)?;
    for op in &ops {
        let drop = |name: &str| format!("DROP INDEX IF EXISTS {}.{}", quote_ident(schema), quote_ident(name));
        match op {
            ManagedOp::Keep(_) => {}
            ManagedOp::Drop(name) => {
                sqlx::query(&drop(name)).execute(&mut *tx).await.map_err(RuntimeError::Schema)?;
            }
            // Create and Replace both drop-then-create: covers a changed spec and
            // adoption of a pre-existing same-named (unmanaged) index alike.
            ManagedOp::Create(name) | ManagedOp::Replace(name) => {
                let idx = by_name[name];
                let create = generate_create_index(schema, &entity.entity_name, idx).map_err(proto)?;
                sqlx::query(&drop(name)).execute(&mut *tx).await.map_err(RuntimeError::Schema)?;
                sqlx::query(&create).execute(&mut *tx).await.map_err(RuntimeError::Schema)?;
                sqlx::query(&format!(
                    "COMMENT ON INDEX {}.{} IS '{}{}'",
                    quote_ident(schema), quote_ident(name), MANAGED_INDEX_PREFIX, index_spec_hash(idx)
                )).execute(&mut *tx).await.map_err(RuntimeError::Schema)?;
            }
        }
    }
    tx.commit().await.map_err(RuntimeError::Schema)?;
    Ok(())
}

/// Drift report for declared indexes (the verify counterpart to reconcile).
/// No introspection needed: `read_managed_index_hashes` returns empty for a
/// nonexistent table (the pg_index join yields nothing), so missing tables
/// naturally produce no drift.
async fn verify_indexes(
    pool: &PgPool,
    app_id: &str,
    entities: &[EntityContract],
) -> Result<Vec<SchemaChange>, RuntimeError> {
    let mut changes = Vec::new();
    for entity in entities {
        let by_name = desired_indexes(entity)
            .map_err(|m| RuntimeError::Schema(sqlx::Error::Protocol(m)))?;
        let desired: Vec<(String, String)> =
            by_name.iter().map(|(n, i)| (n.clone(), index_spec_hash(i))).collect();
        let managed = read_managed_index_hashes(pool, app_id, &entity.entity_name).await?;
        if desired.is_empty() && managed.is_empty() {
            continue;
        }
        for op in compute_managed_diff(&desired, &managed) {
            let change_type = match &op {
                ManagedOp::Keep(_) => continue,
                ManagedOp::Create(_) => "add_index",
                ManagedOp::Replace(_) => "replace_index",
                ManagedOp::Drop(_) => "drop_index",
            };
            changes.push(SchemaChange {
                entity: entity.entity_name.clone(),
                change_type: change_type.to_string(),
                column: op.name().to_string(),
                detail: None,
            });
        }
    }
    Ok(changes)
}

// ── Declarative CHECK constraints ────────────────────────────────────
//
// Same tag-owned model as indexes: we reconcile ONLY constraints carrying a
// `rootcx:chk:<hash>` COMMENT, so app/DBA-defined CHECKs (untagged) are never
// touched. Covers both enum-derived CHECKs (from `enum_values`) and declarative
// `checks:` expressions, unified under one mechanism. The hash is of the
// DECLARED expression (whitespace-normalized) — never Postgres's rewritten
// stored definition — so `a IN (b,c)` → stored `a = ANY(ARRAY[...])` does not
// churn (the trap that Drizzle/TypeORM hit; Atlas avoids it with a dev-database,
// which we don't need because we never compare against the stored text).

const MANAGED_CHECK_PREFIX: &str = "rootcx:chk:";

/// Hash of the DECLARED expression (whitespace-normalized) — never Postgres's
/// rewritten stored definition — so reformatting doesn't churn.
fn check_spec_hash(expr: &str) -> String {
    fnv1a_hex(&expr.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// The boolean expression for a field's enum CHECK, or None if it has no enum.
/// Array columns (TEXT[]) use `<@` (every element in the set); scalars use `IN`.
fn enum_check_expr(field: &FieldContract) -> Option<String> {
    let vals = field.enum_values.as_ref().filter(|v| !v.is_empty())?;
    let col = quote_ident(&field.name);
    let list = vals.iter().map(|v| format!("'{}'", v.replace('\'', "''"))).collect::<Vec<_>>().join(", ");
    Some(match map_field_type(&field.field_type).strip_suffix("[]") {
        Some(elem) => format!("{col} <@ ARRAY[{list}]::{elem}[]"),
        None => format!("{col} IN ({list})"),
    })
}

/// Resolved name → expression for every CHECK the core owns on an entity:
/// enum-derived (`chk_<entity>_<field>`) + declarative (`entity.checks`, explicit
/// name or `chk_<entity>_<hash>`). Rejects duplicate names. Hashes are derived on
/// demand from the expression (mirrors `desired_indexes` returning the spec).
fn desired_checks(entity: &EntityContract) -> Result<HashMap<String, String>, String> {
    let mut by_name: HashMap<String, String> = HashMap::new();
    for f in &entity.fields {
        if let Some(expr) = enum_check_expr(f) {
            let name = format!("chk_{}_{}", entity.entity_name, f.name);
            if by_name.insert(name.clone(), expr).is_some() {
                return Err(format!("duplicate check name '{name}' on '{}'", entity.entity_name));
            }
        }
    }
    for c in &entity.checks {
        let name = c.name.clone().unwrap_or_else(|| format!("chk_{}_{}", entity.entity_name, check_spec_hash(&c.expr)));
        if by_name.insert(name.clone(), c.expr.clone()).is_some() {
            return Err(format!("duplicate check name '{name}' on '{}'", entity.entity_name));
        }
    }
    Ok(by_name)
}

/// name → stored spec-hash for the CHECKs WE manage, from `rootcx:chk:` comments.
/// App/DBA CHECKs carry no such comment, so they never appear and are untouched.
async fn read_managed_check_hashes(
    pool: &PgPool,
    schema: &str,
    table: &str,
) -> Result<HashMap<String, String>, RuntimeError> {
    let rows: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT con.conname, obj_description(con.oid, 'pg_constraint') \
         FROM pg_constraint con \
         JOIN pg_class t ON t.oid = con.conrelid \
         JOIN pg_namespace n ON n.oid = t.relnamespace \
         WHERE n.nspname = $1 AND t.relname = $2 AND con.contype = 'c' \
           AND obj_description(con.oid, 'pg_constraint') LIKE $3",
    )
    .bind(schema).bind(table).bind(format!("{MANAGED_CHECK_PREFIX}%"))
    .fetch_all(pool).await.map_err(RuntimeError::Schema)?;
    Ok(rows
        .into_iter()
        .filter_map(|(name, comment)| {
            comment.and_then(|c| c.strip_prefix(MANAGED_CHECK_PREFIX).map(|h| (name, h.to_string())))
        })
        .collect())
}

/// Reconcile owned CHECKs for one entity: add new, replace changed (drop-first,
/// since `ADD CONSTRAINT` has no `IF NOT EXISTS`), drop removed. Idempotent.
async fn reconcile_checks(
    pool: &PgPool,
    schema: &str,
    entity: &EntityContract,
) -> Result<(), RuntimeError> {
    let proto = |m: String| RuntimeError::Schema(sqlx::Error::Protocol(m));
    let by_name = desired_checks(entity).map_err(proto)?;
    let desired: Vec<(String, String)> =
        by_name.iter().map(|(n, expr)| (n.clone(), check_spec_hash(expr))).collect();
    let managed = read_managed_check_hashes(pool, schema, &entity.entity_name).await?;

    let ops = compute_managed_diff(&desired, &managed);
    if ops.iter().all(|o| matches!(o, ManagedOp::Keep(_))) {
        return Ok(());
    }

    let fq = format!("{}.{}", quote_ident(schema), quote_ident(&entity.entity_name));
    let mut tx = pool.begin().await.map_err(RuntimeError::Schema)?;
    for op in &ops {
        let drop = |name: &str| format!("ALTER TABLE {fq} DROP CONSTRAINT IF EXISTS {}", quote_ident(name));
        match op {
            ManagedOp::Keep(_) => {}
            ManagedOp::Drop(name) => {
                sqlx::query(&drop(name)).execute(&mut *tx).await.map_err(RuntimeError::Schema)?;
            }
            // Drop-then-add: covers a changed expression AND adoption of a
            // pre-existing same-named (untagged) CHECK alike.
            ManagedOp::Create(name) | ManagedOp::Replace(name) => {
                let expr = &by_name[name];
                sqlx::query(&drop(name)).execute(&mut *tx).await.map_err(RuntimeError::Schema)?;
                sqlx::query(&format!("ALTER TABLE {fq} ADD CONSTRAINT {} CHECK ({expr})", quote_ident(name)))
                    .execute(&mut *tx).await.map_err(RuntimeError::Schema)?;
                sqlx::query(&format!(
                    "COMMENT ON CONSTRAINT {} ON {fq} IS '{}{}'",
                    quote_ident(name), MANAGED_CHECK_PREFIX, check_spec_hash(expr)
                )).execute(&mut *tx).await.map_err(RuntimeError::Schema)?;
            }
        }
    }
    tx.commit().await.map_err(RuntimeError::Schema)?;
    Ok(())
}

/// Drift report for owned CHECKs (the verify counterpart to reconcile).
async fn verify_checks(
    pool: &PgPool,
    app_id: &str,
    entities: &[EntityContract],
) -> Result<Vec<SchemaChange>, RuntimeError> {
    let mut changes = Vec::new();
    for entity in entities {
        let by_name = desired_checks(entity)
            .map_err(|m| RuntimeError::Schema(sqlx::Error::Protocol(m)))?;
        let desired: Vec<(String, String)> =
            by_name.iter().map(|(n, expr)| (n.clone(), check_spec_hash(expr))).collect();
        let managed = read_managed_check_hashes(pool, app_id, &entity.entity_name).await?;
        if desired.is_empty() && managed.is_empty() {
            continue;
        }
        for op in compute_managed_diff(&desired, &managed) {
            let change_type = match &op {
                ManagedOp::Keep(_) => continue,
                ManagedOp::Create(_) => "add_check",
                ManagedOp::Replace(_) => "replace_check",
                ManagedOp::Drop(_) => "drop_check",
            };
            changes.push(SchemaChange {
                entity: entity.entity_name.clone(),
                change_type: change_type.to_string(),
                column: op.name().to_string(),
                detail: None,
            });
        }
    }
    Ok(changes)
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

    let rows: Vec<(String, String, bool, Option<String>)> = sqlx::query_as(&format!(
        "SELECT \
            a.attname, \
            format_type(a.atttypid, a.atttypmod), \
            a.attnotnull, \
            pg_get_expr(d.adbin, d.adrelid) \
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

    let fk_rows: Vec<(String, String, String)> = sqlx::query_as(&format!(
        "SELECT a.attname, con.conname, con.confdeltype::text \
         FROM pg_constraint con \
         JOIN pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = ANY(con.conkey) \
         WHERE con.conrelid = '{fq}'::regclass AND con.contype = 'f'"
    ))
    .fetch_all(pool)
    .await
    .map_err(RuntimeError::Schema)?;

    let fk_map: HashMap<String, FkInfo> = fk_rows
        .into_iter()
        .map(|(col, name, rule)| (col, FkInfo { constraint_name: name, delete_rule: rule }))
        .collect();

    Ok(rows
        .into_iter()
        .map(|(name, pg_type, not_null, default_expr)| {
            let fk = fk_map.get(&name).cloned();
            DbColumn { name, pg_type, not_null, default_expr, fk }
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
        ColumnDiff::ReplaceFkConstraint { column_name, delete_rule, .. } => {
            ("alter_fk_delete_rule", column_name, Some(delete_rule.clone()))
        }
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
    changes.extend(verify_indexes(pool, app_id, entities).await?);
    changes.extend(verify_checks(pool, app_id, entities).await?);

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
    use rootcx_types::{CheckContract, FieldContract, FieldReference};
    use serde_json::json;

    fn db_col(name: &str, pg_type: &str, not_null: bool) -> DbColumn {
        DbColumn { name: name.to_string(), pg_type: pg_type.to_string(), not_null, default_expr: None, fk: None }
    }

    fn db_col_with_default(name: &str, pg_type: &str, not_null: bool, default: &str) -> DbColumn {
        DbColumn { name: name.to_string(), pg_type: pg_type.to_string(), not_null, default_expr: Some(default.to_string()), fk: None }
    }

    fn db_col_with_fk(name: &str, pg_type: &str, not_null: bool, fk_name: &str, delete_rule: &str) -> DbColumn {
        DbColumn {
            name: name.to_string(),
            pg_type: pg_type.to_string(),
            not_null,
            default_expr: None,
            fk: Some(FkInfo { constraint_name: fk_name.to_string(), delete_rule: delete_rule.to_string() }),
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
        EntityContract { entity_name: name.to_string(), fields, identity_kind: None, identity_key: None, indexes: vec![], checks: vec![] }
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

    // ── declarative CHECK reconcile (pure logic) ─────────────────────

    #[test]
    fn enum_check_expr_scalar_and_array() {
        let mut scalar = mfield("color", "text");
        scalar.enum_values = Some(vec!["red".into(), "blue".into()]);
        assert_eq!(enum_check_expr(&scalar).unwrap(), r#""color" IN ('red', 'blue')"#);

        let mut arr = mfield("tags", "[text]");
        arr.enum_values = Some(vec!["a".into(), "b".into()]);
        assert_eq!(enum_check_expr(&arr).unwrap(), r#""tags" <@ ARRAY['a', 'b']::TEXT[]"#);

        // single quotes in values are doubled — broken-SQL / injection guard
        let mut quoted = mfield("note", "text");
        quoted.enum_values = Some(vec!["a'b".into()]);
        assert_eq!(enum_check_expr(&quoted).unwrap(), r#""note" IN ('a''b')"#);

        assert!(enum_check_expr(&mfield("plain", "text")).is_none(), "no enum → no check");
        // empty list → no check (guards against invalid `col IN ()`)
        let mut empty = mfield("e", "text");
        empty.enum_values = Some(vec![]);
        assert!(enum_check_expr(&empty).is_none(), "empty enum_values → no check");
    }

    #[test]
    fn desired_checks_merges_enum_and_explicit() {
        let mut gender = mfield("gender", "text");
        gender.enum_values = Some(vec!["m".into(), "f".into()]);
        let mut e = mentity("person", vec![gender]);
        e.checks = vec![CheckContract { name: Some("person_dates_chk".into()), expr: "a >= b".into() }];
        let d = desired_checks(&e).unwrap();
        assert!(d.contains_key("chk_person_gender"), "enum-derived check present");
        assert!(d.contains_key("person_dates_chk"), "explicit check present by name");
        assert_eq!(d["chk_person_gender"], r#""gender" IN ('m', 'f')"#);
    }

    #[test]
    fn desired_checks_auto_names_unnamed() {
        // An explicit check with no name gets a deterministic chk_<entity>_<hash>.
        let mut e = mentity("person", vec![]);
        e.checks = vec![CheckContract { name: None, expr: "x > 0".into() }];
        let d = desired_checks(&e).unwrap();
        let name = d.keys().next().unwrap();
        assert_eq!(name, &format!("chk_person_{}", check_spec_hash("x > 0")));
    }

    #[test]
    fn desired_checks_rejects_duplicate_names() {
        let mut e = mentity("t", vec![]);
        e.checks = vec![
            CheckContract { name: Some("dup".into()), expr: "x".into() },
            CheckContract { name: Some("dup".into()), expr: "y".into() },
        ];
        assert!(desired_checks(&e).is_err(), "duplicate check names must be rejected");
    }

    #[test]
    fn check_spec_hash_ignores_reformatting_only() {
        // No churn on whitespace reformatting; a real expression change → new hash.
        assert_eq!(check_spec_hash("a >= b"), check_spec_hash("a   >=  b"));
        assert_ne!(check_spec_hash("a >= b"), check_spec_hash("a > b"));
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
        // Column-level phases: alter type → add column → nullability → default →
        // drop column. (CHECKs are no longer column-diffs; they reconcile apart.)
        let diff = TableDiff {
            schema_name: "app".into(),
            table_name: "tbl".into(),
            changes: vec![
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
        let alter_type_pos = stmts.iter().position(|s| s.contains("TYPE DOUBLE PRECISION")).unwrap();
        let add_col_pos = stmts.iter().position(|s| s.contains("ADD COLUMN")).unwrap();
        let set_not_null_pos = stmts.iter().position(|s| s.contains("SET NOT NULL")).unwrap();
        let set_default_pos = stmts.iter().position(|s| s.contains("SET DEFAULT")).unwrap();
        let drop_col_pos = stmts.iter().position(|s| s.contains("DROP COLUMN")).unwrap();

        assert!(alter_type_pos < add_col_pos, "ALTER TYPE before ADD COLUMN");
        assert!(add_col_pos < set_not_null_pos, "ADD COLUMN before SET NOT NULL");
        assert!(set_not_null_pos < set_default_pos, "SET NOT NULL before SET DEFAULT");
        assert!(set_default_pos < drop_col_pos, "SET DEFAULT before DROP COLUMN");
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
                ColumnDiff::ReplaceFkConstraint {
                    column_name: "list_id".into(),
                    old_constraint_name: "fk_old".into(),
                    new_constraint_name: "fk_new".into(),
                    target_table: "\"app\".\"lists\"".into(),
                    delete_rule: "CASCADE".into(),
                },
                "alter_fk_delete_rule",
                "list_id",
                Some("CASCADE"),
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

    // ── FK delete rule sync ─────────────────────────────────────────

    fn entity_link_field(name: &str, target: &str, required: bool, on_delete: Option<rootcx_types::OnDeletePolicy>) -> FieldContract {
        FieldContract {
            name: name.to_string(),
            field_type: "entity_link".to_string(),
            required,
            default_value: None,
            enum_values: None,
            references: Some(FieldReference { entity: target.to_string(), field: "id".to_string() }),
            is_primary_key: None,
            on_delete,
        }
    }

    #[test]
    fn diff_fk_delete_rule_change() {
        use rootcx_types::OnDeletePolicy;
        let cases: Vec<(&str, Option<OnDeletePolicy>, bool, &str)> = vec![
            ("n", Some(OnDeletePolicy::Cascade),  false, "CASCADE"),
            ("n", Some(OnDeletePolicy::Restrict), false, "RESTRICT"),
            ("c", Some(OnDeletePolicy::SetNull),  false, "SET NULL"),
            ("c", Some(OnDeletePolicy::Restrict), true,  "RESTRICT"),
            ("r", Some(OnDeletePolicy::Cascade),  true,  "CASCADE"),
            ("n", None,                           true,  "RESTRICT"),
            ("r", None,                           false, "SET NULL"),
        ];
        for (db_rule, on_delete, required, expected_clause) in &cases {
            let db = vec![db_col_with_fk("list_id", "uuid", *required, "fk_app_items_list_id_lists", db_rule)];
            let manifest = vec![entity_link_field("list_id", "lists", *required, *on_delete)];
            let mut pk = HashMap::new();
            pk.insert("lists".to_string(), "UUID" as &str);
            let diff = compute_diff("app", "items", &db, &manifest, &pk);
            let fk_change = diff.changes.iter().find(|c| matches!(c, ColumnDiff::ReplaceFkConstraint { .. }));
            assert!(fk_change.is_some(),
                "db_rule={db_rule}, on_delete={on_delete:?}, required={required}: expected ReplaceFkConstraint");
            match fk_change.unwrap() {
                ColumnDiff::ReplaceFkConstraint { delete_rule, old_constraint_name, .. } => {
                    assert_eq!(delete_rule, *expected_clause,
                        "db_rule={db_rule}, on_delete={on_delete:?}: expected {expected_clause}");
                    assert_eq!(old_constraint_name, "fk_app_items_list_id_lists");
                }
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn diff_fk_delete_rule_no_change() {
        use rootcx_types::OnDeletePolicy;
        let cases: Vec<(&str, Option<OnDeletePolicy>, bool)> = vec![
            ("c", Some(OnDeletePolicy::Cascade), true),
            ("n", Some(OnDeletePolicy::SetNull), false),
            ("r", Some(OnDeletePolicy::Restrict), true),
            ("n", None, false),
            ("r", None, true),
        ];
        for (db_rule, on_delete, required) in &cases {
            let db = vec![db_col_with_fk("list_id", "uuid", *required, "fk_app_items_list_id_lists", db_rule)];
            let manifest = vec![entity_link_field("list_id", "lists", *required, *on_delete)];
            let mut pk = HashMap::new();
            pk.insert("lists".to_string(), "UUID" as &str);
            let diff = compute_diff("app", "items", &db, &manifest, &pk);
            let fk_changes: Vec<_> = diff.changes.iter()
                .filter(|c| matches!(c, ColumnDiff::ReplaceFkConstraint { .. }))
                .collect();
            assert!(fk_changes.is_empty(),
                "db_rule={db_rule}, on_delete={on_delete:?}, required={required}: expected no FK change, got: {fk_changes:?}");
        }
    }

    #[test]
    fn diff_fk_no_existing_fk_skips() {
        use rootcx_types::OnDeletePolicy;
        let db = vec![db_col("list_id", "uuid", true)];
        let manifest = vec![entity_link_field("list_id", "lists", true, Some(OnDeletePolicy::Cascade))];
        let mut pk = HashMap::new();
        pk.insert("lists".to_string(), "UUID" as &str);
        let diff = compute_diff("app", "items", &db, &manifest, &pk);
        assert!(
            !diff.changes.iter().any(|c| matches!(c, ColumnDiff::ReplaceFkConstraint { .. })),
            "no existing FK in DB → should not produce ReplaceFkConstraint"
        );
    }

    // ── declarative indexes ──────────────────────────────────────────

    use rootcx_types::{IndexColumn, IndexColumnSpec, IndexContract};

    fn idx(columns: Vec<IndexColumn>) -> IndexContract {
        IndexContract { name: None, columns, unique: false, using: None, where_clause: None, with: Default::default() }
    }
    fn col(name: &str) -> IndexColumn { IndexColumn::Name(name.into()) }
    fn spec(s: IndexColumnSpec) -> IndexColumn { IndexColumn::Spec(s) }
    fn blank_spec() -> IndexColumnSpec {
        IndexColumnSpec { column: None, expr: None, sort: None, nulls: None, ops: None }
    }

    #[test]
    fn index_simple_composite() {
        let sql = generate_create_index("crm", "person", &idx(vec![col("last_name"), col("first_name")])).unwrap();
        assert!(sql.starts_with("CREATE INDEX "), "{sql}");
        assert!(sql.contains("ON \"crm\".\"person\" USING btree (\"last_name\", \"first_name\")"), "{sql}");
        assert!(!sql.contains("UNIQUE"), "{sql}");
    }

    #[test]
    fn index_unique_partial_named() {
        let mut i = idx(vec![col("email")]);
        i.unique = true;
        i.name = Some("person_email_uq".into());
        i.where_clause = Some("email IS NOT NULL".into());
        let sql = generate_create_index("crm", "person", &i).unwrap();
        assert!(sql.contains("CREATE UNIQUE INDEX \"person_email_uq\""), "{sql}");
        assert!(sql.ends_with("WHERE email IS NOT NULL"), "{sql}");
    }

    #[test]
    fn index_functional_gin_with_storage() {
        let mut i = idx(vec![spec(IndexColumnSpec { expr: Some("lower(last_name)".into()), ..blank_spec() })]);
        i.using = Some("gin".into());
        i.with.insert("fillfactor".into(), "70".into());
        let sql = generate_create_index("crm", "person", &i).unwrap();
        assert!(sql.contains("USING gin ((lower(last_name)))"), "{sql}");
        assert!(sql.contains("WITH (fillfactor = 70)"), "{sql}");
    }

    #[test]
    fn index_sort_nulls_ops() {
        let i = idx(vec![spec(IndexColumnSpec {
            column: Some("created_at".into()), sort: Some("desc".into()),
            nulls: Some("last".into()), ops: Some("text_ops".into()), ..blank_spec()
        })]);
        let sql = generate_create_index("crm", "person", &i).unwrap();
        assert!(sql.contains("(\"created_at\" text_ops DESC NULLS LAST)"), "{sql}");
    }

    #[test]
    fn index_rejects_bad_vocab_and_shapes() {
        // bad method
        let mut bad = idx(vec![col("x")]); bad.using = Some("bogus".into());
        assert!(generate_create_index("crm", "t", &bad).is_err());
        // bad sort
        let s = idx(vec![spec(IndexColumnSpec { column: Some("x".into()), sort: Some("sideways".into()), ..blank_spec() })]);
        assert!(generate_create_index("crm", "t", &s).is_err());
        // both column and expr
        let both = idx(vec![spec(IndexColumnSpec { column: Some("x".into()), expr: Some("f(x)".into()), ..blank_spec() })]);
        assert!(generate_create_index("crm", "t", &both).is_err());
        // neither
        let neither = idx(vec![spec(blank_spec())]);
        assert!(generate_create_index("crm", "t", &neither).is_err());
        // no columns
        assert!(generate_create_index("crm", "t", &idx(vec![])).is_err());
    }

    #[test]
    fn index_spec_hash_reflects_definition() {
        let base = idx(vec![col("a"), col("b")]);
        let h = index_spec_hash(&base);
        assert_eq!(h, index_spec_hash(&base.clone()), "same spec → same hash");
        // every definitional axis must move the hash
        let mut uq = base.clone(); uq.unique = true;
        assert_ne!(index_spec_hash(&uq), h, "unique changes hash");
        let mut cols = base.clone(); cols.columns = vec![col("a")];
        assert_ne!(index_spec_hash(&cols), h, "columns change hash");
        let mut whr = base.clone(); whr.where_clause = Some("a IS NOT NULL".into());
        assert_ne!(index_spec_hash(&whr), h, "where changes hash");
        let mut using = base.clone(); using.using = Some("gin".into());
        assert_ne!(index_spec_hash(&using), h, "method changes hash");
        let mut with = base.clone(); with.with.insert("fillfactor".into(), "70".into());
        assert_ne!(index_spec_hash(&with), h, "storage params change hash");
        // name is NOT part of the hash (it's the key, not the definition)
        let mut named = base.clone(); named.name = Some("whatever".into());
        assert_eq!(index_spec_hash(&named), h, "name is not part of the spec hash");
    }

    #[test]
    fn index_diff_add_keep_change_remove() {
        let h = |s: &str| s.to_string();
        // add: desired present, nothing managed
        assert_eq!(
            compute_managed_diff(&[("a".into(), h("1"))], &HashMap::new()),
            vec![ManagedOp::Create("a".into())]
        );
        // keep: same name + same hash
        assert_eq!(
            compute_managed_diff(&[("a".into(), h("1"))], &HashMap::from([("a".into(), h("1"))])),
            vec![ManagedOp::Keep("a".into())]
        );
        // change: same name, different hash → replace
        assert_eq!(
            compute_managed_diff(&[("a".into(), h("2"))], &HashMap::from([("a".into(), h("1"))])),
            vec![ManagedOp::Replace("a".into())]
        );
        // remove: managed, no longer desired
        assert_eq!(
            compute_managed_diff(&[], &HashMap::from([("a".into(), h("1"))])),
            vec![ManagedOp::Drop("a".into())]
        );
    }

    #[test]
    fn index_diff_mixed() {
        let ops = compute_managed_diff(
            &[("keep".into(), "h".into()), ("changed".into(), "new".into()), ("added".into(), "h".into())],
            &HashMap::from([
                ("keep".into(), "h".into()),
                ("changed".into(), "old".into()),
                ("removed".into(), "h".into()),
            ]),
        );
        assert!(ops.contains(&ManagedOp::Keep("keep".into())));
        assert!(ops.contains(&ManagedOp::Replace("changed".into())));
        assert!(ops.contains(&ManagedOp::Create("added".into())));
        assert!(ops.contains(&ManagedOp::Drop("removed".into())));
        assert_eq!(ops.len(), 4);
    }

    #[test]
    fn index_name_derivation_deterministic_and_capped() {
        let i = idx(vec![col("last_name"), col("first_name")]);
        let a = resolve_index_name("person", &i);
        let b = resolve_index_name("person", &i);
        assert_eq!(a, b, "auto name must be deterministic");
        assert!(a.starts_with("ix_person_last_name_first_name"), "{a}");
        assert!(a.len() <= 63, "must respect pg identifier limit: {a}");
        // explicit name wins
        let mut named = i.clone(); named.name = Some("custom".into());
        assert_eq!(resolve_index_name("person", &named), "custom");
        // unique changes the derived name
        let mut uq = i.clone(); uq.unique = true;
        assert_ne!(resolve_index_name("person", &uq), a);
    }

    #[test]
    fn ddl_replace_fk_constraint() {
        let diff = TableDiff {
            schema_name: "app".into(),
            table_name: "items".into(),
            changes: vec![ColumnDiff::ReplaceFkConstraint {
                column_name: "list_id".into(),
                old_constraint_name: "fk_old".into(),
                new_constraint_name: "fk_new".into(),
                target_table: "\"app\".\"lists\"".into(),
                delete_rule: "CASCADE".into(),
            }],
        };
        let stmts = generate_ddl(&diff);
        assert_eq!(stmts.len(), 2, "expected DROP + ADD: {stmts:?}");
        assert!(stmts[0].contains("DROP CONSTRAINT IF EXISTS") && stmts[0].contains("\"fk_old\""),
            "first stmt should drop old FK: {}", stmts[0]);
        assert!(stmts[1].contains("ADD CONSTRAINT") && stmts[1].contains("ON DELETE CASCADE"),
            "second stmt should create new FK: {}", stmts[1]);
    }

}
