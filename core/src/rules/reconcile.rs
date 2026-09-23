//! Reconcile compiled rules with the database without ever leaving a declared
//! rule unenforced (ADR 0010).
//!
//! Core owns only objects carrying its `rootcx:chk:`/`rootcx:idx:` comment. One
//! install runs on one dedicated session with a pinned search path and timeouts:
//!
//! 1. drop leftovers of an interrupted run (pending tags, `rootcx_next_*`,
//!    invalid indexes under a managed name);
//! 2. keep objects whose tag matches; adopt legacy objects by retagging;
//! 3. preflight every new or changed rule against existing rows, for the whole
//!    app, before changing anything;
//! 4. create new rules `NOT VALID` then validate; replace changed ones through
//!    a validated `rootcx_next_*` twin swapped in one transaction;
//! 5. drop managed objects the manifest no longer declares, last.

use std::collections::HashMap;

use sha2::{Digest, Sha256};
use sqlx::{Connection, PgConnection, PgPool};
use tracing::info;

use super::{CompiledCheck, CompiledIndex, Origin, compile_entity};
use crate::RuntimeError;
use crate::manifest::{quote_ident, quote_literal};
use rootcx_types::{AppManifest, EntityContract, SchemaChange};

const CHECK_TAG: &str = "rootcx:chk:";
const INDEX_TAG: &str = "rootcx:idx:";
const PENDING: &str = "pending-";
const NEXT_PREFIX: &str = "rootcx_next_";
/// A rule's validation scans its table; a long one is legitimate, a stuck
/// lock is not.
const SESSION: [&str; 4] = [
    "SET search_path = pg_catalog, pg_temp",
    "SET standard_conforming_strings = on",
    "SET lock_timeout = '30s'",
    "SET statement_timeout = '15min'",
];

#[derive(Debug, Clone, PartialEq, Eq)]
enum Plan {
    Keep,
    /// A legacy object proven equivalent: rewrite its tag, no table scan.
    Adopt,
    Create,
    /// Replace a differently-tagged or untagged object of the same name.
    Replace,
}

struct Existing {
    comment: Option<String>,
    valid: bool,
}

struct Table<'a> {
    schema: &'a str,
    /// Where pg_trgm lives, when the app declares trigram indexes.
    trigram: Option<String>,
    entity: &'a EntityContract,
    checks: Vec<(CompiledCheck, Plan)>,
    indexes: Vec<(CompiledIndex, Plan)>,
    drop_checks: Vec<String>,
    drop_indexes: Vec<String>,
    leftover_checks: Vec<String>,
    leftover_indexes: Vec<String>,
}

impl Table<'_> {
    fn fq(&self) -> String {
        format!("{}.{}", quote_ident(self.schema), quote_ident(&self.entity.entity_name))
    }
}

fn schema_error(e: sqlx::Error) -> RuntimeError {
    RuntimeError::Schema(e)
}

/// The manifest stored before this install. Its declarations decide whether an
/// object an older Core created may be adopted without a scan.
async fn previous_entities(pool: &PgPool, app_id: &str) -> Result<Vec<EntityContract>, RuntimeError> {
    let stored = crate::manifest::load_manifest_json(pool, app_id).await?;
    Ok(stored
        .and_then(|json| serde_json::from_value::<AppManifest>(json).ok())
        .map(|manifest| manifest.data_contract)
        .unwrap_or_default())
}

async fn existing_checks(conn: &mut PgConnection, schema: &str, table: &str) -> Result<HashMap<String, Existing>, RuntimeError> {
    let rows: Vec<(String, Option<String>, bool)> = sqlx::query_as(
        "SELECT con.conname::text, obj_description(con.oid, 'pg_constraint'), con.convalidated
         FROM pg_constraint con JOIN pg_class t ON t.oid = con.conrelid
         JOIN pg_namespace n ON n.oid = t.relnamespace
         WHERE n.nspname = $1 AND t.relname = $2 AND con.contype = 'c'",
    ).bind(schema).bind(table).fetch_all(&mut *conn).await.map_err(schema_error)?;
    Ok(rows.into_iter().map(|(name, comment, valid)| (name, Existing { comment, valid })).collect())
}

async fn existing_indexes(conn: &mut PgConnection, schema: &str, table: &str) -> Result<HashMap<String, Existing>, RuntimeError> {
    let rows: Vec<(String, Option<String>, bool)> = sqlx::query_as(
        "SELECT c.relname::text, obj_description(c.oid, 'pg_class'), x.indisvalid
         FROM pg_index x JOIN pg_class c ON c.oid = x.indexrelid
         JOIN pg_class t ON t.oid = x.indrelid JOIN pg_namespace n ON n.oid = t.relnamespace
         WHERE n.nspname = $1 AND t.relname = $2",
    ).bind(schema).bind(table).fetch_all(&mut *conn).await.map_err(schema_error)?;
    Ok(rows.into_iter().map(|(name, comment, valid)| (name, Existing { comment, valid })).collect())
}

/// Whether an existing trigram index already has exactly the declared shape:
/// valid GIN over the same plain columns with the installed pg_trgm's operator
/// class, and no predicate.
async fn trigram_matches(conn: &mut PgConnection, schema: &str, name: &str, columns: &[String], trigram: &str) -> Result<bool, RuntimeError> {
    let shape: Option<(String, bool, Vec<String>, Vec<String>, Vec<String>)> = sqlx::query_as(
        "SELECT am.amname::text, x.indpred IS NULL,
                ARRAY(SELECT a.attname::text FROM unnest(x.indkey) WITH ORDINALITY k(attnum, i)
                      JOIN pg_attribute a ON a.attrelid = x.indrelid AND a.attnum = k.attnum ORDER BY k.i),
                ARRAY(SELECT o.opcname::text FROM unnest(x.indclass) WITH ORDINALITY k(oid, i)
                      JOIN pg_opclass o ON o.oid = k.oid ORDER BY k.i),
                ARRAY(SELECT ns.nspname::text FROM unnest(x.indclass) WITH ORDINALITY k(oid, i)
                      JOIN pg_opclass o ON o.oid = k.oid JOIN pg_namespace ns ON ns.oid = o.opcnamespace ORDER BY k.i)
         FROM pg_index x JOIN pg_class c ON c.oid = x.indexrelid JOIN pg_am am ON am.oid = c.relam
         JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE n.nspname = $1 AND c.relname = $2 AND x.indisvalid AND x.indexprs IS NULL",
    ).bind(schema).bind(name).fetch_optional(&mut *conn).await.map_err(schema_error)?;
    Ok(shape.is_some_and(|(am, no_predicate, keys, opclasses, namespaces)| {
        am == "gin" && no_predicate && keys == columns
            && opclasses.iter().all(|o| o == "gin_trgm_ops")
            && namespaces.iter().all(|n| n == trigram)
    }))
}

fn tag_of<'a>(comment: &'a Option<String>, prefix: &str) -> Option<&'a str> {
    comment.as_deref().and_then(|c| c.strip_prefix(prefix))
}

/// The previous manifest declared this check with the same source, so the
/// object an older Core compiled from it means what the new compilation means.
fn check_was_declared(previous: Option<&EntityContract>, current: &EntityContract, check: &CompiledCheck) -> bool {
    let Some(previous) = previous else { return false };
    match &check.origin {
        Origin::Check { expr } => previous.checks.iter()
            .any(|c| c.name.as_deref() == Some(check.name.as_str()) && &c.expr == expr),
        Origin::Enum { field } => {
            let find = |e: &EntityContract| e.fields.iter().find(|f| &f.name == field)
                .map(|f| (f.field_type.clone(), f.enum_values.clone()));
            find(previous).is_some() && find(previous) == find(current)
        }
        Origin::Shorthand => false,
    }
}

fn index_was_declared(previous: Option<&EntityContract>, index: &CompiledIndex) -> bool {
    let Some(previous) = previous else { return false };
    let declared = serde_json::to_value(&index.declared).ok();
    previous.indexes.iter().any(|i| {
        crate::schema_sync::resolve_index_name(&previous.entity_name, i) == index.name
            && serde_json::to_value(i).ok() == declared
    })
}

fn declares_trigram(entities: &[EntityContract]) -> bool {
    entities.iter().flat_map(|e| &e.indexes)
        .any(|i| i.using.as_deref().is_some_and(|u| u.eq_ignore_ascii_case("trigram")))
}

/// The schema of the installed pg_trgm. Existing installations keep it where
/// it is: app SQL may call its functions by qualified name.
async fn trigram_schema(conn: &mut PgConnection) -> Result<Option<String>, RuntimeError> {
    sqlx::query_scalar(
        "SELECT n.nspname::text FROM pg_extension e JOIN pg_namespace n ON n.oid = e.extnamespace
         WHERE e.extname = 'pg_trgm'",
    ).fetch_optional(&mut *conn).await.map_err(schema_error)
}

/// Install pg_trgm into the Core-owned schema when no app has installed it.
async fn ensure_trigram(conn: &mut PgConnection) -> Result<String, RuntimeError> {
    if let Some(schema) = trigram_schema(conn).await? {
        return Ok(schema);
    }
    let schema = quote_ident(super::EXTENSION_SCHEMA);
    execute(conn, &[
        format!("CREATE SCHEMA IF NOT EXISTS {schema}"),
        format!("CREATE EXTENSION IF NOT EXISTS pg_trgm WITH SCHEMA {schema}"),
    ]).await?;
    Ok(super::EXTENSION_SCHEMA.to_string())
}

fn next_name(name: &str) -> String {
    format!("{NEXT_PREFIX}{}", hex::encode(&Sha256::digest(name.as_bytes())[..8]))
}

async fn plan<'a>(
    conn: &mut PgConnection,
    schema: &'a str,
    entity: &'a EntityContract,
    previous: Option<&EntityContract>,
    trigram: Option<&str>,
) -> Result<Table<'a>, RuntimeError> {
    let rules = compile_entity(entity).map_err(|e| RuntimeError::Invalid(format!(
        "rules for '{schema}.{}': {e}", entity.entity_name
    )))?;
    let checks_now = existing_checks(conn, schema, &entity.entity_name).await?;
    let indexes_now = existing_indexes(conn, schema, &entity.entity_name).await?;
    let mut table = Table {
        schema, trigram: trigram.map(str::to_string), entity, checks: vec![], indexes: vec![],
        drop_checks: vec![], drop_indexes: vec![], leftover_checks: vec![], leftover_indexes: vec![],
    };

    for (name, existing) in &checks_now {
        let pending = tag_of(&existing.comment, CHECK_TAG).is_some_and(|t| t.starts_with(PENDING));
        if pending || name.starts_with(NEXT_PREFIX) {
            table.leftover_checks.push(name.clone());
        }
    }
    for (name, existing) in &indexes_now {
        let ours = name.starts_with(NEXT_PREFIX) || rules.indexes.iter().any(|i| &i.name == name)
            || tag_of(&existing.comment, INDEX_TAG).is_some();
        if name.starts_with(NEXT_PREFIX) || (ours && !existing.valid) {
            table.leftover_indexes.push(name.clone());
        }
    }

    for check in rules.checks {
        let plan = match checks_now.get(&check.name).filter(|_| !table.leftover_checks.contains(&check.name)) {
            None => Plan::Create,
            Some(existing) => match tag_of(&existing.comment, CHECK_TAG) {
                Some(tag) if tag == check.tag && existing.valid => Plan::Keep,
                Some(tag) if existing.valid && check.legacy_tag.as_deref() == Some(tag)
                    && check_was_declared(previous, entity, &check) => Plan::Adopt,
                _ => Plan::Replace,
            },
        };
        table.checks.push((check, plan));
    }
    for index in rules.indexes {
        let existing = indexes_now.get(&index.name).filter(|_| !table.leftover_indexes.contains(&index.name));
        let tag = existing.map(|e| tag_of(&e.comment, INDEX_TAG).map(str::to_string));
        let plan = match tag {
            None => Plan::Create,
            Some(Some(tag)) if tag == index.tag => Plan::Keep,
            Some(Some(tag)) if tag == index.legacy_tag && index_was_declared(previous, &index) => Plan::Adopt,
            Some(_) => {
                let same_shape = match (index.trigram, trigram) {
                    (true, Some(trigram)) => trigram_matches(conn, schema, &index.name, &index.columns, trigram).await?,
                    _ => false,
                };
                if same_shape { Plan::Adopt } else { Plan::Replace }
            }
        };
        table.indexes.push((index, plan));
    }

    for (name, existing) in &checks_now {
        let managed = tag_of(&existing.comment, CHECK_TAG).is_some();
        if managed && !table.checks.iter().any(|(c, _)| &c.name == name) && !table.leftover_checks.contains(name) {
            table.drop_checks.push(name.clone());
        }
    }
    for (name, existing) in &indexes_now {
        let managed = tag_of(&existing.comment, INDEX_TAG).is_some();
        if managed && !table.indexes.iter().any(|(i, _)| &i.name == name) && !table.leftover_indexes.contains(name) {
            table.drop_indexes.push(name.clone());
        }
    }
    Ok(table)
}

/// Refuse the install, changing nothing, if existing rows violate a new or
/// changed rule. `early` runs before the install adds or retypes columns: a
/// rule that cannot be evaluated yet is skipped and checked again once they
/// exist.
async fn preflight_table(conn: &mut PgConnection, table: &Table<'_>, early: bool) -> Result<(), RuntimeError> {
    let fq = table.fq();
    let entity = &table.entity.entity_name;
    for (check, plan) in &table.checks {
        if !matches!(plan, Plan::Create | Plan::Replace) {
            continue;
        }
        let found: Result<(i64, Option<Vec<String>>), sqlx::Error> = sqlx::query_as(&format!(
            "SELECT count(*), (array_agg(id::text ORDER BY id))[1:5] FROM {fq} WHERE ({}) IS FALSE",
            check.sql,
        )).fetch_one(&mut *conn).await;
        let (count, ids) = match found {
            Err(error) if early && not_evaluable_yet(&error) => continue,
            found => found.map_err(schema_error)?,
        };
        if count > 0 {
            return Err(RuntimeError::Invalid(format!(
                "rule '{}' on '{entity}' is violated by {count} existing row{} (ids {}); nothing was changed",
                check.name, if count == 1 { "" } else { "s" }, ids.unwrap_or_default().join(", "),
            )));
        }
    }
    for (index, plan) in &table.indexes {
        if !index.unique || !matches!(plan, Plan::Create | Plan::Replace) {
            continue;
        }
        let keys = index.key_values.join(", ");
        let mut filter = index.key_values.iter().map(|k| format!("{k} IS NOT NULL")).collect::<Vec<_>>().join(" AND ");
        if let Some(predicate) = &index.predicate {
            filter.push_str(&format!(" AND {predicate}"));
        }
        let found: Result<i64, sqlx::Error> = sqlx::query_scalar(&format!(
            "SELECT count(*) FROM (SELECT 1 FROM {fq} WHERE {filter} GROUP BY {keys} HAVING count(*) > 1) duplicates",
        )).fetch_one(&mut *conn).await;
        let groups = match found {
            Err(error) if early && not_evaluable_yet(&error) => continue,
            found => found.map_err(schema_error)?,
        };
        if groups > 0 {
            return Err(RuntimeError::Invalid(format!(
                "unique index '{}' on '{entity}' would reject {groups} group{} of duplicate existing rows; nothing was changed",
                index.name, if groups == 1 { "" } else { "s" },
            )));
        }
    }
    Ok(())
}

/// Missing columns or types the install has yet to change (SQLSTATE class 42).
fn not_evaluable_yet(error: &sqlx::Error) -> bool {
    error.as_database_error().and_then(|e| e.code()).is_some_and(|code| code.starts_with("42"))
}

async fn execute(conn: &mut PgConnection, statements: &[String]) -> Result<(), RuntimeError> {
    let mut tx = conn.begin().await.map_err(schema_error)?;
    for statement in statements {
        sqlx::query(statement).execute(&mut *tx).await.map_err(schema_error)?;
    }
    tx.commit().await.map_err(schema_error)
}

fn comment_check(fq: &str, name: &str, tag: &str) -> String {
    format!("COMMENT ON CONSTRAINT {} ON {fq} IS {}", quote_ident(name), quote_literal(&format!("{CHECK_TAG}{tag}")))
}

fn comment_index(schema: &str, name: &str, tag: &str) -> String {
    format!("COMMENT ON INDEX {}.{} IS {}", quote_ident(schema), quote_ident(name), quote_literal(&format!("{INDEX_TAG}{tag}")))
}

/// Add a check `NOT VALID` under a pending tag and commit, then validate it
/// under a lock that allows writes. Returns once the constraint is enforced for
/// every row.
async fn add_validated_check(conn: &mut PgConnection, fq: &str, name: &str, check: &CompiledCheck) -> Result<(), RuntimeError> {
    execute(conn, &[
        format!("ALTER TABLE {fq} ADD CONSTRAINT {} CHECK ({}) NOT VALID", quote_ident(name), check.sql),
        comment_check(fq, name, &format!("{PENDING}{}", check.tag)),
    ]).await?;
    let validated = sqlx::query(&format!("ALTER TABLE {fq} VALIDATE CONSTRAINT {}", quote_ident(name)))
        .execute(&mut *conn).await;
    if let Err(error) = validated {
        let _ = sqlx::query(&format!("ALTER TABLE {fq} DROP CONSTRAINT IF EXISTS {}", quote_ident(name)))
            .execute(&mut *conn).await;
        return Err(match error.as_database_error().and_then(|e| e.code()).as_deref() {
            Some("23514") => RuntimeError::Invalid(format!(
                "rule '{}' is violated by rows written during the install; nothing was changed", check.name
            )),
            _ => schema_error(error),
        });
    }
    Ok(())
}

/// Build an index. Non-empty tables are indexed concurrently so writes
/// continue; a failed concurrent build leaves an invalid index, which is
/// dropped here and would otherwise be cleaned by the next install.
async fn build_index(conn: &mut PgConnection, table: &Table<'_>, index: &CompiledIndex, name: &str) -> Result<(), RuntimeError> {
    let has_rows: bool = sqlx::query_scalar(&format!("SELECT EXISTS (SELECT 1 FROM {})", table.fq()))
        .fetch_one(&mut *conn).await.map_err(schema_error)?;
    let trigram = table.trigram.as_deref().unwrap_or(super::EXTENSION_SCHEMA);
    let sql = index.create_sql(table.schema, &table.entity.entity_name, name, has_rows, trigram);
    let built = sqlx::query(&sql).execute(&mut *conn).await;
    if let Err(error) = built {
        let _ = sqlx::query(&format!("DROP INDEX IF EXISTS {}.{}", quote_ident(table.schema), quote_ident(name)))
            .execute(&mut *conn).await;
        return Err(match error.as_database_error().and_then(|e| e.code()).as_deref() {
            Some("23505") => RuntimeError::Invalid(format!(
                "unique index '{}' is violated by rows written during the install; nothing was changed", index.name
            )),
            _ => schema_error(error),
        });
    }
    Ok(())
}

async fn apply(conn: &mut PgConnection, table: &Table<'_>) -> Result<(), RuntimeError> {
    let fq = table.fq();
    let schema = table.schema;
    for (check, plan) in &table.checks {
        match plan {
            Plan::Keep => {}
            Plan::Adopt => execute(conn, &[comment_check(&fq, &check.name, &check.tag)]).await?,
            Plan::Create => {
                add_validated_check(conn, &fq, &check.name, check).await?;
                execute(conn, &[comment_check(&fq, &check.name, &check.tag)]).await?;
            }
            Plan::Replace => {
                let next = next_name(&check.name);
                add_validated_check(conn, &fq, &next, check).await?;
                execute(conn, &[
                    format!("ALTER TABLE {fq} DROP CONSTRAINT {}", quote_ident(&check.name)),
                    format!("ALTER TABLE {fq} RENAME CONSTRAINT {} TO {}", quote_ident(&next), quote_ident(&check.name)),
                    comment_check(&fq, &check.name, &check.tag),
                ]).await?;
            }
        }
    }
    for (index, plan) in &table.indexes {
        match plan {
            Plan::Keep => {}
            Plan::Adopt => execute(conn, &[comment_index(schema, &index.name, &index.tag)]).await?,
            Plan::Create => {
                build_index(conn, table, index, &index.name).await?;
                execute(conn, &[comment_index(schema, &index.name, &index.tag)]).await?;
            }
            Plan::Replace => {
                let next = next_name(&index.name);
                build_index(conn, table, index, &next).await?;
                execute(conn, &[
                    format!("DROP INDEX {}.{}", quote_ident(schema), quote_ident(&index.name)),
                    format!("ALTER INDEX {}.{} RENAME TO {}", quote_ident(schema), quote_ident(&next), quote_ident(&index.name)),
                    comment_index(schema, &index.name, &index.tag),
                ]).await?;
            }
        }
    }
    Ok(())
}

async fn drop_objects(conn: &mut PgConnection, table: &Table<'_>, checks: &[String], indexes: &[String]) -> Result<(), RuntimeError> {
    let fq = table.fq();
    let mut statements: Vec<String> = checks.iter()
        .map(|name| format!("ALTER TABLE {fq} DROP CONSTRAINT IF EXISTS {}", quote_ident(name)))
        .collect();
    statements.extend(indexes.iter()
        .map(|name| format!("DROP INDEX IF EXISTS {}.{}", quote_ident(table.schema), quote_ident(name))));
    if statements.is_empty() {
        return Ok(());
    }
    execute(conn, &statements).await
}

/// Move extensions out of an app schema about to be dropped: `DROP SCHEMA …
/// CASCADE` would otherwise drop the extension and every other app's index
/// that uses its operator classes.
pub(crate) async fn evacuate_extensions(pool: &PgPool, app_id: &str) -> Result<(), RuntimeError> {
    let extensions: Vec<(String, bool)> = sqlx::query_as(
        "SELECT e.extname::text, e.extrelocatable FROM pg_extension e
         JOIN pg_namespace n ON n.oid = e.extnamespace WHERE n.nspname = $1",
    ).bind(app_id).fetch_all(pool).await.map_err(schema_error)?;
    for (name, relocatable) in extensions {
        if !relocatable {
            return Err(RuntimeError::Conflict(format!(
                "extension '{name}' is installed in the schema of '{app_id}' and cannot be moved; \
                 an operator must relocate or drop it before uninstalling"
            )));
        }
        let mut tx = pool.begin().await.map_err(schema_error)?;
        sqlx::query(&format!("CREATE SCHEMA IF NOT EXISTS {}", quote_ident(super::EXTENSION_SCHEMA)))
            .execute(&mut *tx).await.map_err(schema_error)?;
        sqlx::query(&format!("ALTER EXTENSION {} SET SCHEMA {}", quote_ident(&name), quote_ident(super::EXTENSION_SCHEMA)))
            .execute(&mut *tx).await.map_err(schema_error)?;
        tx.commit().await.map_err(schema_error)?;
        info!(app = %app_id, extension = %name, "extension moved out of the app schema before uninstall");
    }
    Ok(())
}

/// Reconcile every entity's rules for one install, on one dedicated session.
pub(crate) async fn reconcile(pool: &PgPool, app_id: &str, entities: &[EntityContract]) -> Result<(), RuntimeError> {
    let previous = previous_entities(pool, app_id).await?;
    let mut conn = pool.acquire().await.map_err(schema_error)?.detach();
    let result = reconcile_on(&mut conn, app_id, entities, &previous).await;
    let _ = conn.close().await;
    result
}

async fn reconcile_on(
    conn: &mut PgConnection,
    app_id: &str,
    entities: &[EntityContract],
    previous: &[EntityContract],
) -> Result<(), RuntimeError> {
    for setting in SESSION {
        sqlx::query(setting).execute(&mut *conn).await.map_err(schema_error)?;
    }
    let trigram = match declares_trigram(entities) {
        true => Some(ensure_trigram(conn).await?),
        false => None,
    };
    let mut tables = Vec::with_capacity(entities.len());
    for entity in entities {
        let before = previous.iter().find(|e| e.entity_name == entity.entity_name);
        tables.push(plan(conn, app_id, entity, before, trigram.as_deref()).await?);
    }
    for table in &tables {
        drop_objects(conn, table, &table.leftover_checks, &table.leftover_indexes).await?;
    }
    for table in &tables {
        preflight_table(conn, table, false).await?;
    }
    for table in &tables {
        apply(conn, table).await?;
    }
    for table in &tables {
        drop_objects(conn, table, &table.drop_checks, &table.drop_indexes).await?;
    }
    let mut counts = [0usize; 4];
    for table in &tables {
        let plans = table.checks.iter().map(|(_, p)| p).chain(table.indexes.iter().map(|(_, p)| p));
        for plan in plans {
            counts[match plan { Plan::Keep => 0, Plan::Adopt => 1, Plan::Create => 2, Plan::Replace => 3 }] += 1;
        }
    }
    let dropped: usize = tables.iter().map(|t| t.drop_checks.len() + t.drop_indexes.len()).sum();
    info!(
        app = %app_id, kept = counts[0], adopted = counts[1], created = counts[2], replaced = counts[3],
        dropped, "rules reconciled"
    );
    Ok(())
}

/// Read-only preflight for an install, run before Core revokes anything for the
/// new manifest, so data that violates a new rule refuses the deploy without
/// touching the running installation.
pub(crate) async fn preflight(pool: &PgPool, app_id: &str, entities: &[EntityContract]) -> Result<(), RuntimeError> {
    let previous = previous_entities(pool, app_id).await?;
    let mut conn = pool.acquire().await.map_err(schema_error)?;
    let conn = &mut *conn;
    let tables: Vec<String> = sqlx::query_scalar("SELECT tablename::text FROM pg_tables WHERE schemaname = $1")
        .bind(app_id).fetch_all(&mut *conn).await.map_err(schema_error)?;
    let trigram = trigram_schema(conn).await?;
    for entity in entities.iter().filter(|e| tables.contains(&e.entity_name)) {
        let before = previous.iter().find(|e| e.entity_name == entity.entity_name);
        let table = plan(conn, app_id, entity, before, trigram.as_deref()).await?;
        preflight_table(conn, &table, true).await?;
    }
    Ok(())
}

/// What an install of `entities` would change, for the verify endpoint.
pub(crate) async fn verify(pool: &PgPool, app_id: &str, entities: &[EntityContract]) -> Result<Vec<SchemaChange>, RuntimeError> {
    let previous = previous_entities(pool, app_id).await?;
    let mut conn = pool.acquire().await.map_err(schema_error)?;
    let conn = &mut *conn;
    let trigram = trigram_schema(conn).await?;
    let mut changes = Vec::new();
    for entity in entities {
        let before = previous.iter().find(|e| e.entity_name == entity.entity_name);
        let table = plan(conn, app_id, entity, before, trigram.as_deref()).await?;
        let change = |kind: &str, name: &str| SchemaChange {
            entity: entity.entity_name.clone(), change_type: kind.into(), column: name.into(), detail: None,
        };
        for (check, plan) in &table.checks {
            match plan {
                Plan::Keep => {}
                Plan::Adopt => changes.push(change("adopt_check", &check.name)),
                Plan::Create => changes.push(change("add_check", &check.name)),
                Plan::Replace => changes.push(change("replace_check", &check.name)),
            }
        }
        for (index, plan) in &table.indexes {
            match plan {
                Plan::Keep => {}
                Plan::Adopt => changes.push(change("adopt_index", &index.name)),
                Plan::Create => changes.push(change("add_index", &index.name)),
                Plan::Replace => changes.push(change("replace_index", &index.name)),
            }
        }
        changes.extend(table.drop_checks.iter().chain(&table.leftover_checks).map(|n| change("drop_check", n)));
        changes.extend(table.drop_indexes.iter().chain(&table.leftover_indexes).map(|n| change("drop_index", n)));
    }
    Ok(changes)
}
