//! Reading the catalog and executing the row-access plan. All SQL composition
//! lives in `super::plan`; this module supplies it with facts and runs what it
//! returns.

use sqlx::PgConnection;

use crate::RuntimeError;
use super::exec;
use super::plan::{LinkFacts, OwnerFacts, TableFacts};

/// Enable + FORCE RLS on an app table, grant the restricted executor CRUD on it,
/// and (re)create its permission-gated policies. Idempotent: safe to call on every
/// install and on the retroactive boot pass. `schema`/`table` are validated
/// snake_case identifiers (see `manifest::validate_manifest`).
///
/// `owners` describes ownership for every entity of the schema, since resolving a
/// delegated one needs its ancestors too. A table absent from the map gets exactly
/// the pre-ownership SQL, so a schema that declares nothing is untouched.
///
/// The whole plan is computed before its first statement runs, so a contract that
/// cannot compile aborts without having applied any of it.
pub(crate) async fn apply_table_rls(
    conn: &mut PgConnection,
    schema: &str,
    table: &str,
    owners: &OwnerMap,
    shared: Option<&str>,
    sensitive: &[String],
    approved_read: bool,
) -> Result<(), RuntimeError> {
    let facts = TableFacts {
        columns: columns(conn, schema, table).await?,
        owner: owner_facts(conn, schema, table, owners).await?,
    };
    for statement in super::plan::table_rls(schema, table, &facts, shared, sensitive, approved_read)? {
        exec(conn, &statement).await?;
    }
    Ok(())
}

async fn columns(conn: &mut PgConnection, schema: &str, table: &str) -> Result<Vec<String>, RuntimeError> {
    sqlx::query_scalar(
        "SELECT attname FROM pg_attribute WHERE attrelid = to_regclass($1)
         AND attnum > 0 AND NOT attisdropped ORDER BY attnum",
    )
    .bind(qualified(schema, table))
    .fetch_all(conn).await.map_err(RuntimeError::Schema)
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
/// again where the name is actually resolved (`owner_facts`, which fails closed).
pub(crate) fn fits_ident_limit(name: &str) -> bool { name.len() <= 63 }

/// Walk from `table` to the entity holding a real user id. Pure: the chain is a
/// property of the manifest alone.
///
/// Keeps compilation bounded even after manifest/projection validation: an
/// unresolvable chain must fail during reconciliation, before a caller queries the
/// table. Returns an empty chain when the entity declares no ownership.
fn owner_chain<'a>(
    schema: &str,
    table: &'a str,
    owners: &'a OwnerMap,
) -> Result<Vec<(&'a str, &'a str)>, RuntimeError> {
    let mut chain: Vec<(&str, &str)> = Vec::new();
    let mut current = table;
    loop {
        let Some((column, parent)) = owners.get(current) else {
            if chain.is_empty() { return Ok(chain); }
            return Err(RuntimeError::Invalid(format!("governance: '{schema}.{table}' delegates to '{current}' with no owner")));
        };
        if chain.iter().any(|(entity, _)| *entity == current) {
            return Err(RuntimeError::Invalid(format!("governance: ownership of '{schema}.{table}' loops at '{current}'")));
        }
        chain.push((current, column.as_str()));
        let Some(parent) = parent else { break };
        if chain.len() >= crate::manifest::MAX_OWNER_CHAIN {
            return Err(RuntimeError::Invalid(format!("governance: ownership of '{schema}.{table}' exceeds MAX_OWNER_CHAIN")));
        }
        current = parent.as_str();
    }
    Ok(chain)
}

/// Resolve the chain's catalog types, root first. None means no ownership was
/// declared; a declared ownership that cannot be resolved is an installation/boot
/// error.
async fn owner_facts(
    conn: &mut PgConnection,
    schema: &str,
    table: &str,
    owners: &OwnerMap,
) -> Result<Option<OwnerFacts>, RuntimeError> {
    let chain = owner_chain(schema, table, owners)?;
    if chain.is_empty() { return Ok(None) }

    let (root, root_column) = chain[chain.len() - 1];
    let Some(root_type) = column_type(conn, schema, root, root_column).await? else {
        return Err(RuntimeError::Invalid(format!("governance: owner column '{schema}.{root}.{root_column}' is missing")));
    };
    if !matches!(root_type.as_str(), "uuid" | "text") {
        return Err(RuntimeError::Invalid(format!("governance: owner column '{schema}.{root}.{root_column}' must be uuid or text, found {root_type}")));
    }

    // Descend back towards `table`: one resolver per link crossed, in the order
    // the plan must declare them.
    let mut links = Vec::new();
    for index in (0..chain.len() - 1).rev() {
        let (parent, _) = chain[index + 1];
        let (entity, link) = chain[index];
        let (Some(pk), Some(_link_type)) = (
            primary_key(conn, schema, parent).await?,
            column_type(conn, schema, entity, link).await?,
        ) else {
            return Err(RuntimeError::Invalid(format!("governance: link '{schema}.{entity}.{link}' or primary key of '{parent}' is missing")));
        };
        let Some(pk_type) = column_type(conn, schema, parent, &pk).await? else {
            return Err(RuntimeError::Invalid(format!("governance: key '{schema}.{parent}.{pk}' is missing")));
        };
        let resolver = owner_resolver_name(schema, parent);
        if !fits_ident_limit(&resolver) {
            return Err(RuntimeError::Invalid(format!("governance: resolver '{resolver}' exceeds PostgreSQL identifier limit")));
        }
        links.push(LinkFacts { parent: parent.into(), link: link.into(), resolver, pk, pk_type });
    }

    Ok(Some(OwnerFacts { root_column: root_column.into(), root_type, links }))
}

/// Drop the schema's resolvers for entities no longer named in `keep`. Called once
/// the app's policies are current, so nothing dropped can still be referenced.
pub(super) async fn prune_owner_resolvers_tx(
    conn: &mut PgConnection,
    schema: &str,
    keep: &[String],
) -> Result<(), RuntimeError> {
    let existing: Vec<String> = sqlx::query_scalar(
        "SELECT p.proname FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace \
          WHERE n.nspname = 'rootcx_system' AND p.proname LIKE 'rootcx\\_own.%'",
    ).fetch_all(&mut *conn).await.map_err(RuntimeError::Schema)?;

    for name in existing {
        let Some((owner_schema, entity)) = name["rootcx_own.".len()..].split_once('.') else { continue };
        if owner_schema != schema || keep.iter().any(|k| k == entity) { continue }
        exec(conn, &format!(
            "DROP FUNCTION IF EXISTS rootcx_system.{}()", crate::manifest::quote_ident(&name),
        )).await?;
    }
    Ok(())
}

pub(super) async fn column_type(
    conn: &mut PgConnection,
    schema: &str,
    table: &str,
    column: &str,
) -> Result<Option<String>, RuntimeError> {
    sqlx::query_scalar::<_, String>(
        "SELECT format_type(a.atttypid, a.atttypmod) FROM pg_attribute a
          WHERE a.attrelid = to_regclass($1) AND a.attname = $2 AND a.attnum > 0 AND NOT a.attisdropped",
    )
    .bind(qualified(schema, table))
    .bind(column)
    .fetch_optional(conn).await.map_err(RuntimeError::Schema)
}

/// The single-column primary key a delegation link points at. Read from the
/// catalog rather than assumed to be `id`, since an entity may name its own.
pub(super) async fn primary_key(conn: &mut PgConnection, schema: &str, table: &str) -> Result<Option<String>, RuntimeError> {
    sqlx::query_scalar::<_, String>(
        "SELECT a.attname FROM pg_index i
           JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = i.indkey[0]
          WHERE i.indrelid = to_regclass($1) AND i.indisprimary AND i.indnatts = 1",
    )
    .bind(qualified(schema, table))
    .fetch_optional(conn).await.map_err(RuntimeError::Schema)
}

fn qualified(schema: &str, table: &str) -> String {
    format!("{}.{}", crate::manifest::quote_ident(schema), crate::manifest::quote_ident(table))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owners(pairs: &[(&str, &str, Option<&str>)]) -> OwnerMap {
        pairs.iter()
            .map(|(e, c, p)| ((*e).into(), ((*c).into(), p.map(Into::into))))
            .collect()
    }

    #[test]
    fn an_undeclared_entity_has_no_chain() {
        assert!(owner_chain("hr", "profile", &owners(&[])).unwrap().is_empty());
    }

    #[test]
    fn a_chain_runs_from_the_table_to_its_root() {
        let map = owners(&[("task", "project_id", Some("project")), ("project", "owner_id", None)]);
        assert_eq!(owner_chain("hr", "task", &map).unwrap(), vec![("task", "project_id"), ("project", "owner_id")]);
    }

    #[test]
    fn a_loop_is_refused() {
        let map = owners(&[("a", "b_id", Some("b")), ("b", "a_id", Some("a"))]);
        assert!(owner_chain("hr", "a", &map).unwrap_err().to_string().contains("loops at"));
    }

    #[test]
    fn delegating_to_an_entity_without_an_owner_is_refused() {
        let map = owners(&[("task", "project_id", Some("project"))]);
        assert!(owner_chain("hr", "task", &map).unwrap_err().to_string().contains("with no owner"));
    }
}
