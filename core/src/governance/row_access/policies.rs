use sqlx::PgConnection;

use crate::RuntimeError;
use super::exec;

/// Enable + FORCE RLS on an app table, grant the restricted executor CRUD on it,
/// and (re)create its permission-gated policies. Idempotent: safe to call on every
/// install and on the retroactive boot pass. `schema`/`table` are validated
/// snake_case identifiers (see `manifest::validate_manifest`).
///
/// `owners` describes ownership for every entity of the schema, since resolving a
/// delegated one needs its ancestors too. A table absent from the map gets exactly
/// the pre-ownership SQL, so a schema that declares nothing is untouched.
pub(crate) async fn apply_table_rls(
    conn: &mut PgConnection,
    schema: &str,
    table: &str,
    owners: &OwnerMap,
    shared: Option<&str>,
    sensitive: &[String],
    approved_read: bool,
) -> Result<(), RuntimeError> {
    use crate::manifest::quote_ident;
    let qt = format!("{}.{}", quote_ident(schema), quote_ident(table));

    exec(conn, &format!("GRANT USAGE ON SCHEMA {} TO rootcx_app_executor", quote_ident(schema))).await?;
    let columns: Vec<String> = sqlx::query_scalar(
        "SELECT attname FROM pg_attribute WHERE attrelid = to_regclass($1)
         AND attnum > 0 AND NOT attisdropped ORDER BY attnum",
    ).bind(&qt).fetch_all(&mut *conn).await.map_err(RuntimeError::Schema)?;
    for field in sensitive {
        if !columns.contains(field) {
            return Err(RuntimeError::Invalid(format!("governance: sensitive column '{schema}.{table}.{field}' is missing")));
        }
    }
    // Table grants override column restrictions. Revoke both historic table and
    // column grants before rebuilding exactly the readable set.
    exec(conn, &format!("REVOKE SELECT ON {qt} FROM PUBLIC, rootcx_app_executor")).await?;
    let all = columns.iter().map(|c| quote_ident(c)).collect::<Vec<_>>().join(", ");
    if !all.is_empty() {
        exec(conn, &format!("REVOKE SELECT ({all}) ON {qt} FROM PUBLIC, rootcx_app_executor")).await?;
    }
    let readable = columns.iter().filter(|c| !sensitive.contains(c))
        .map(|c| quote_ident(c)).collect::<Vec<_>>().join(", ");
    if sensitive.is_empty() {
        exec(conn, &format!("GRANT SELECT ON {qt} TO rootcx_app_executor")).await?;
    } else if !readable.is_empty() {
        exec(conn, &format!("GRANT SELECT ({readable}) ON {qt} TO rootcx_app_executor")).await?;
    }
    exec(conn, &format!("GRANT INSERT, UPDATE, DELETE ON {qt} TO rootcx_app_executor")).await?;
    exec(conn, &format!(
        "ALTER DEFAULT PRIVILEGES IN SCHEMA {} REVOKE ALL ON TABLES FROM PUBLIC, rootcx_app_executor",
        quote_ident(schema),
    )).await?;
    exec(conn, &format!("ALTER TABLE {qt} ENABLE ROW LEVEL SECURITY")).await?;
    exec(conn, &format!("ALTER TABLE {qt} FORCE ROW LEVEL SECURITY")).await?;

    // "This row is mine", or None when there is no owner to compare against — in
    // which case every `_own` policy below is dropped and not recreated, so removing
    // the declaration removes the confinement instead of stranding it.
    let mine = owner_predicate(conn, schema, table, owners).await?;
    let shared = shared.map(|predicate| format!(
        "{} AND {predicate}", gate(&format!("app:{schema}:{table}.read.shared")),
    ));
    set_policy(
        conn, &qt, "rootcx_rls_select_shared", "SELECT", &["USING"], shared.as_deref(), false,
    ).await?;

    for (policy, command, action, clauses) in RLS_POLICIES {
        let key = format!("app:{schema}:{table}.{action}");
        let predicate = collection_gate(&key, mine.as_deref());
        set_policy(
            conn, &qt, policy, command, clauses,
            Some(&predicate), false,
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
            conn, &qt, &format!("{policy}_own"), command, clauses, scoped.as_deref(), false,
        ).await?;

        // Even a provider's additional permissive policy cannot widen a public
        // execution. Other restrictive provider policies continue to apply.
        let public = if action == "read" {
            publication_gate(&key, mine.as_deref(), owners.contains_key(table))
        } else {
            "FALSE".into()
        };
        let ceiling = format!(
            "coalesce(current_setting('rootcx.publication_id', true), '') = '' OR ({public})"
        );
        set_policy(
            conn, &qt, &format!("{policy}_publication_ceiling"), command, clauses,
            Some(&ceiling), true,
        ).await?;
    }

    let public = publication_gate(
        &format!("app:{schema}:{table}.read"), mine.as_deref(), owners.contains_key(table),
    );
    set_policy(conn, &qt, "rootcx_rls_select_publication", "SELECT", &["USING"], Some(&public), false).await?;

    // PostgreSQL row locks require UPDATE privilege and its USING predicate.
    // An approved reader may lock a row, but never pass a write's WITH CHECK.
    exec(conn, &format!("DROP POLICY IF EXISTS rootcx_rls_action_lock ON {qt}")).await?;
    if approved_read {
        let read_key = crate::manifest::quote_literal(&format!("app:{schema}:{table}.read"));
        exec(conn, &format!(
            "CREATE POLICY rootcx_rls_action_lock ON {qt} FOR UPDATE \
             USING ((SELECT rootcx_system.check_approved_action_access({read_key}))) WITH CHECK (FALSE)"
        )).await?;
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

fn cross_gate(key: &str) -> String {
    format!(
        "(SELECT rootcx_system.check_cross_app_access({}))",
        crate::manifest::quote_literal(key),
    )
}

fn collection_gate(key: &str, mine: Option<&str>) -> String {
    let base = gate(key);
    let cross = cross_gate(key);
    match mine {
        Some(mine) => {
            let cross_own = cross_gate(&format!("{key}.{}", crate::manifest::OWN_SCOPE));
            format!("({base} OR (({cross} OR {cross_own}) AND {mine}))")
        }
        None => format!("({base} OR {cross})"),
    }
}

fn publication_gate(key: &str, mine: Option<&str>, has_owner: bool) -> String {
    let gate = format!(
        "(SELECT rootcx_system.check_publication_access({}))",
        crate::manifest::quote_literal(key),
    );
    if !has_owner {
        return gate;
    }
    format!(
        "{gate} AND (current_setting('rootcx.publication_release_ownership', true) = '1' OR ({}))",
        mine.unwrap_or("FALSE"),
    )
}

/// (Re)define one policy, or drop it when there is no predicate. Dropped first
/// either way, so every call is a redefinition rather than a duplicate — which is
/// what makes `apply_table_rls` safe to run on each install and on every boot.
async fn set_policy(
    conn: &mut PgConnection,
    qt: &str,
    name: &str,
    command: &str,
    clauses: &[&str],
    predicate: Option<&str>,
    restrictive: bool,
) -> Result<(), RuntimeError> {
    exec(conn, &format!("DROP POLICY IF EXISTS {name} ON {qt}")).await?;
    let Some(predicate) = predicate else { return Ok(()) };
    let body = clauses
        .iter()
        .map(|clause| format!("{clause} ({predicate})"))
        .collect::<Vec<_>>()
        .join(" ");
    let mode = if restrictive { "AS RESTRICTIVE" } else { "" };
    exec(conn, &format!("CREATE POLICY {name} ON {qt} {mode} FOR {command} {body}")).await
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

/// "This row is mine", as SQL. None means no ownership was declared; a declared
/// ownership that cannot be compiled is an installation/boot error.
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
/// alone and cuts the recursion at a function boundary. Keep the historical
/// `= ANY (ARRAY(...))` ownership predicate, which can support indexed membership
/// when the surrounding plan allows it. Shared reads use their own hashed set.
///
/// Corrupt projections and missing columns abort reconciliation atomically.
async fn owner_predicate(
    conn: &mut PgConnection,
    schema: &str,
    table: &str,
    owners: &OwnerMap,
) -> Result<Option<String>, RuntimeError> {
    // Walk to the entity that holds a real user id. Keep compilation bounded
    // even after manifest/projection validation: an unresolvable chain must fail
    // during reconciliation, before a caller queries the table.
    let mut chain: Vec<(&str, &str)> = Vec::new();
    let mut current = table;
    loop {
        let Some((column, parent)) = owners.get(current) else {
            if chain.is_empty() { return Ok(None); }
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

    let (root, root_column) = chain[chain.len() - 1];
    let Some(root_type) = column_type(conn, schema, root, root_column).await? else {
        return Err(RuntimeError::Invalid(format!("governance: owner column '{schema}.{root}.{root_column}' is missing")));
    };
    if !matches!(root_type.as_str(), "uuid" | "text") {
        return Err(RuntimeError::Invalid(format!("governance: owner column '{schema}.{root}.{root_column}' must be uuid or text, found {root_type}")));
    }
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
        declare_owner_resolver(conn, schema, parent, &resolver, &pk, &pk_type, &mine).await?;
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
/// from that role, so an app cannot claim to be another. Core's direct human
/// data requests may resolve ownership across apps for linked/federated reads;
/// only the trusted HTTP transaction constructor can set that marker.
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
    conn: &mut PgConnection,
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
    let approved_read = quote_literal(&format!("app:{schema}:{entity}.read"));
    // STABLE, never IMMUTABLE: the answer depends on the caller's GUCs and on the
    // table, so an IMMUTABLE marking would let the planner constant-fold one
    // caller's reachable set into a cached plan and serve it to every other user —
    // a permanent cross-user leak. `resolvers_are_stable_secdef_and_not_public`
    // (tests/row_ownership_test.rs) asserts the volatility in the catalog.
    exec(conn, &format!(
        "CREATE OR REPLACE FUNCTION {signature} RETURNS SETOF {pk_type} \
         LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $rootcx$ \
         SELECT {} FROM {}.{} \
          WHERE (current_setting('rootcx.human_data_request', true) = '1' \
                 OR coalesce(nullif(current_setting('rootcx.app_id', true), ''), {own_app}) = {own_app}) \
            AND (coalesce(current_setting('rootcx.approved_action_id', true), '') = '' \
                 OR rootcx_system.check_approved_action_access({approved_read})) \
            AND {mine} $rootcx$",
        quote_ident(pk), quote_ident(schema), quote_ident(entity),
    )).await?;
    // A new function is executable by PUBLIC by default, and this one names a
    // specific user's rows.
    exec(conn, &format!("REVOKE ALL ON FUNCTION {signature} FROM PUBLIC")).await?;
    exec(conn, &format!("GRANT EXECUTE ON FUNCTION {signature} TO rootcx_app_executor")).await
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

    /// Every minted key must have a policy enforcing it, and every policy a key
    /// that reaches it. The two lists exist separately because one maps SQL
    /// commands and the other names permissions, so nothing but this stops a
    /// fifth action being added to one and silently missing from the other.
    #[test]
    fn minted_actions_match_the_policies() {
        let mut minted = crate::extensions::rbac::ENTITY_ACTIONS;
        let mut gated = RLS_POLICIES.map(|(_, _, action, _)| action);
        minted.sort_unstable();
        gated.sort_unstable();
        assert_eq!(minted, gated);
    }

    #[test]
    fn cross_app_actions_are_conjoined_with_ownership_when_present() {
        for action in crate::extensions::rbac::ENTITY_ACTIONS {
            let key = format!("app:hr:profile.{action}");
            let owned = collection_gate(&key, Some("owner_predicate"));
            assert!(owned.contains("check_access"), "{action}");
            assert!(owned.contains("check_cross_app_access"), "{action}");
            assert!(owned.contains(&format!("{key}.own")), "{action}");
            assert!(owned.contains("AND owner_predicate"), "{action}");

            let unowned = collection_gate(&key, None);
            assert!(unowned.contains("check_cross_app_access"), "{action}");
            assert!(!unowned.contains(".own"), "{action}");
            assert!(!unowned.contains("AND owner_predicate"), "{action}");
        }
    }
}
