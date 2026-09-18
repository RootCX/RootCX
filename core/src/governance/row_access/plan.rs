//! The row-access contract as statements, computed without a database.
//!
//! Every SQL string a table's access rules need is built here from catalog facts
//! supplied by the caller, so "this contract produces these rules" is answerable
//! in a unit test rather than by diffing `pg_policies` after a boot. The applier
//! (`super::policies`) reads the facts and executes what this module returns, in
//! order; it composes no SQL of its own.

use crate::RuntimeError;
use crate::manifest::{quote_ident, quote_literal};

/// The catalog answers a table's rules depend on. Read once, before planning.
pub(crate) struct TableFacts {
    /// Every live column, in `attnum` order.
    pub(crate) columns: Vec<String>,
    /// None when the entity declares no ownership, which is also what tells a
    /// publication it has no ownership to re-check.
    pub(crate) owner: Option<OwnerFacts>,
}

/// A resolved ownership chain, root first, with the catalog types its predicates
/// cast against.
pub(crate) struct OwnerFacts {
    pub(crate) root_column: String,
    pub(crate) root_type: String,
    /// One per delegation link crossed, in the order the resolvers must be
    /// declared: from the link nearest the root down to the table itself.
    pub(crate) links: Vec<LinkFacts>,
}

/// One delegation link: the child column `link` points at `parent`'s `pk`.
pub(crate) struct LinkFacts {
    pub(crate) parent: String,
    pub(crate) link: String,
    pub(crate) resolver: String,
    pub(crate) pk: String,
    pub(crate) pk_type: String,
}

/// The policies every app table carries, one per SQL command: the policy name,
/// the command, the permission action gating it, and the clauses it needs.
///
/// UPDATE is the only command taking its predicate twice — USING picks the rows it
/// may touch, WITH CHECK vets the row it leaves behind. Both are load-bearing for
/// a row-scoped policy: USING alone would still let a confined caller hand its own
/// row to somebody else.
pub(super) const RLS_POLICIES: [(&str, &str, &str, &[&str]); 4] = [
    ("rootcx_rls_select", "SELECT", "read", &["USING"]),
    ("rootcx_rls_insert", "INSERT", "create", &["WITH CHECK"]),
    ("rootcx_rls_delete", "DELETE", "delete", &["USING"]),
    ("rootcx_rls_update", "UPDATE", "update", &["USING", "WITH CHECK"]),
];

/// Every statement that brings one table's access rules to their declared state,
/// in execution order. Idempotent: each policy is dropped before it is recreated,
/// which is what makes a replay at boot safe.
pub(crate) fn table_rls(
    schema: &str,
    table: &str,
    facts: &TableFacts,
    shared: Option<&str>,
    sensitive: &[String],
    approved_read: bool,
) -> Result<Vec<String>, RuntimeError> {
    for field in sensitive {
        if !facts.columns.contains(field) {
            return Err(RuntimeError::Invalid(format!(
                "governance: sensitive column '{schema}.{table}.{field}' is missing"
            )));
        }
    }

    let qt = format!("{}.{}", quote_ident(schema), quote_ident(table));
    let mut out = Vec::new();
    out.push(format!("GRANT USAGE ON SCHEMA {} TO rootcx_app_executor", quote_ident(schema)));
    privileges(&mut out, schema, &qt, &facts.columns, sensitive);
    out.push(format!("ALTER TABLE {qt} ENABLE ROW LEVEL SECURITY"));
    out.push(format!("ALTER TABLE {qt} FORCE ROW LEVEL SECURITY"));

    // "This row is mine", or None when there is no owner to compare against — in
    // which case every `_own` policy below is dropped and not recreated, so removing
    // the declaration removes the confinement instead of stranding it.
    let mine = facts.owner.as_ref().map(|owner| ownership(&mut out, schema, owner));

    let shared = shared.map(|predicate| format!(
        "{} AND {predicate}", gate(&format!("app:{schema}:{table}.read.shared")),
    ));
    policy(&mut out, &qt, "rootcx_rls_select_shared", "SELECT", &["USING"], shared.as_deref());

    for (name, command, action, clauses) in RLS_POLICIES {
        let key = format!("app:{schema}:{table}.{action}");
        policy(&mut out, &qt, name, command, clauses, Some(&collection_gate(&key, mine.as_deref())));

        // The row-scoped twin. PERMISSIVE, so Postgres ORs it with the unscoped
        // policy above: access becomes `unscoped OR (scoped AND mine)`, which only
        // ever *adds* what a `.own` holder can reach and leaves every existing
        // grant bit-identical. RESTRICTIVE would AND instead, and lock every app
        // already in production out of its own data.
        let scoped = mine.as_ref().map(|mine| {
            let own_key = format!("app:{schema}:{table}.{action}.{}", crate::manifest::OWN_SCOPE);
            format!("{} AND {mine}", gate(&own_key))
        });
        policy(&mut out, &qt, &format!("{name}_own"), command, clauses, scoped.as_deref());

        // Even a provider's additional permissive policy cannot widen a public
        // execution. Other restrictive provider policies continue to apply.
        let public = if action == "read" {
            publication_gate(&key, mine.as_deref())
        } else {
            "FALSE".into()
        };
        let ceiling = format!(
            "coalesce(current_setting('rootcx.publication_id', true), '') = '' OR ({public})"
        );
        restrictive_policy(&mut out, &qt, &format!("{name}_publication_ceiling"), command, clauses, &ceiling);
    }

    let public = publication_gate(&format!("app:{schema}:{table}.read"), mine.as_deref());
    policy(&mut out, &qt, "rootcx_rls_select_publication", "SELECT", &["USING"], Some(&public));

    // PostgreSQL row locks require UPDATE privilege and its USING predicate.
    // An approved reader may lock a row, but never pass a write's WITH CHECK.
    out.push(format!("DROP POLICY IF EXISTS rootcx_rls_action_lock ON {qt}"));
    if approved_read {
        let read_key = quote_literal(&format!("app:{schema}:{table}.read"));
        out.push(format!(
            "CREATE POLICY rootcx_rls_action_lock ON {qt} FOR UPDATE \
             USING ((SELECT rootcx_system.check_approved_action_access({read_key}))) WITH CHECK (FALSE)"
        ));
    }
    Ok(out)
}

/// Rebuild exactly the executor's readable column set, then its write grants.
/// Table grants override column restrictions, so historic grants of both shapes
/// are revoked first.
fn privileges(out: &mut Vec<String>, schema: &str, qt: &str, columns: &[String], sensitive: &[String]) {
    out.push(format!("REVOKE SELECT ON {qt} FROM PUBLIC, rootcx_app_executor"));
    let all = columns.iter().map(|c| quote_ident(c)).collect::<Vec<_>>().join(", ");
    if !all.is_empty() {
        out.push(format!("REVOKE SELECT ({all}) ON {qt} FROM PUBLIC, rootcx_app_executor"));
    }
    let readable = columns.iter().filter(|c| !sensitive.contains(c))
        .map(|c| quote_ident(c)).collect::<Vec<_>>().join(", ");
    if sensitive.is_empty() {
        out.push(format!("GRANT SELECT ON {qt} TO rootcx_app_executor"));
    } else if !readable.is_empty() {
        out.push(format!("GRANT SELECT ({readable}) ON {qt} TO rootcx_app_executor"));
    }
    out.push(format!("GRANT INSERT, UPDATE, DELETE ON {qt} TO rootcx_app_executor"));
    out.push(format!(
        "ALTER DEFAULT PRIVILEGES IN SCHEMA {} REVOKE ALL ON TABLES FROM PUBLIC, rootcx_app_executor",
        quote_ident(schema),
    ));
}

/// "This row is mine", as SQL, emitting one resolver per delegation link crossed.
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
/// (see `resolver`), which makes ownership a fact about the data alone and cuts
/// the recursion at a function boundary. Keep the historical `= ANY (ARRAY(...))`
/// ownership predicate, which can support indexed membership when the surrounding
/// plan allows it. Shared reads use their own hashed set.
///
/// Resolvers are emitted before the policies that name them, so a child's policy
/// can never be created before its resolver exists.
fn ownership(out: &mut Vec<String>, schema: &str, owner: &OwnerFacts) -> String {
    let mut mine = format!(
        "{} = (SELECT nullif(current_setting('rootcx.user_id', true), ''))::{}",
        quote_ident(&owner.root_column), owner.root_type,
    );
    for link in &owner.links {
        resolver(out, schema, link, &mine);
        mine = format!(
            "{} = ANY (ARRAY(SELECT rootcx_system.{}()))",
            quote_ident(&link.link), quote_ident(&link.resolver),
        );
    }
    mine
}

/// The set of primary keys of a parent entity the caller owns.
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
fn resolver(out: &mut Vec<String>, schema: &str, link: &LinkFacts, mine: &str) {
    let signature = format!("rootcx_system.{}()", quote_ident(&link.resolver));
    let own_app = quote_literal(schema);
    let approved_read = quote_literal(&format!("app:{schema}:{}.read", link.parent));
    // STABLE, never IMMUTABLE: the answer depends on the caller's GUCs and on the
    // table, so an IMMUTABLE marking would let the planner constant-fold one
    // caller's reachable set into a cached plan and serve it to every other user —
    // a permanent cross-user leak. `resolvers_are_stable_secdef_and_not_public`
    // (tests/row_ownership_test.rs) asserts the volatility in the catalog.
    out.push(format!(
        "CREATE OR REPLACE FUNCTION {signature} RETURNS SETOF {} \
         LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $rootcx$ \
         SELECT {} FROM {}.{} \
          WHERE (current_setting('rootcx.human_data_request', true) = '1' \
                 OR coalesce(nullif(current_setting('rootcx.app_id', true), ''), {own_app}) = {own_app}) \
            AND (coalesce(current_setting('rootcx.approved_action_id', true), '') = '' \
                 OR rootcx_system.check_approved_action_access({approved_read})) \
            AND {mine} $rootcx$",
        link.pk_type,
        quote_ident(&link.pk), quote_ident(schema), quote_ident(&link.parent),
    ));
    // A new function is executable by PUBLIC by default, and this one names a
    // specific user's rows.
    out.push(format!("REVOKE ALL ON FUNCTION {signature} FROM PUBLIC"));
    out.push(format!("GRANT EXECUTE ON FUNCTION {signature} TO rootcx_app_executor"));
}

/// (Re)define one PERMISSIVE policy, or drop it when there is no predicate.
/// Dropped first either way, so every call is a redefinition rather than a
/// duplicate — which is what makes the plan safe to replay on each install and on
/// every boot.
fn policy(out: &mut Vec<String>, qt: &str, name: &str, command: &str, clauses: &[&str], predicate: Option<&str>) {
    emit(out, qt, name, command, clauses, predicate, "");
}

/// A ceiling: Postgres ANDs it with every permissive policy, so no additional
/// policy a provider declares can widen past it.
fn restrictive_policy(out: &mut Vec<String>, qt: &str, name: &str, command: &str, clauses: &[&str], predicate: &str) {
    emit(out, qt, name, command, clauses, Some(predicate), "AS RESTRICTIVE");
}

fn emit(
    out: &mut Vec<String>,
    qt: &str,
    name: &str,
    command: &str,
    clauses: &[&str],
    predicate: Option<&str>,
    mode: &str,
) {
    out.push(format!("DROP POLICY IF EXISTS {name} ON {qt}"));
    let Some(predicate) = predicate else { return };
    let body = clauses
        .iter()
        .map(|clause| format!("{clause} ({predicate})"))
        .collect::<Vec<_>>()
        .join(" ");
    out.push(format!("CREATE POLICY {name} ON {qt} {mode} FOR {command} {body}"));
}

/// A permission requirement as an RLS predicate. The `(SELECT ...)` wrapper makes
/// the planner evaluate `check_access` once per query (its arguments are constant)
/// instead of once per row — mandatory for perf, not cosmetic.
fn gate(key: &str) -> String {
    format!("(SELECT rootcx_system.check_access({}))", quote_literal(key))
}

fn cross_gate(key: &str) -> String {
    format!("(SELECT rootcx_system.check_cross_app_access({}))", quote_literal(key))
}

pub(super) fn collection_gate(key: &str, mine: Option<&str>) -> String {
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

/// An owned entity keeps confining a public read to the caller's own rows unless
/// the publication was explicitly approved to release ownership.
fn publication_gate(key: &str, mine: Option<&str>) -> String {
    let gate = format!(
        "(SELECT rootcx_system.check_publication_access({}))", quote_literal(key),
    );
    let Some(mine) = mine else { return gate };
    format!(
        "{gate} AND (current_setting('rootcx.publication_release_ownership', true) = '1' OR ({mine}))"
    )
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

    fn facts(columns: &[&str], owner: Option<OwnerFacts>) -> TableFacts {
        TableFacts { columns: columns.iter().map(|c| (*c).to_string()).collect(), owner }
    }

    fn direct(column: &str, ty: &str) -> OwnerFacts {
        OwnerFacts { root_column: column.into(), root_type: ty.into(), links: Vec::new() }
    }

    #[test]
    fn a_sensitive_column_absent_from_the_table_is_refused() {
        let err = table_rls("hr", "profile", &facts(&["id"], None), None, &["salary".into()], false)
            .unwrap_err().to_string();
        assert!(err.contains("sensitive column 'hr.profile.salary' is missing"), "{err}");
    }

    /// The executor's readable set is rebuilt column by column once anything is
    /// withheld, and the withheld column never appears in a GRANT.
    #[test]
    fn a_sensitive_column_is_withheld_from_the_executor() {
        let plan = table_rls("hr", "profile", &facts(&["id", "salary"], None), None, &["salary".into()], false).unwrap();
        assert!(plan.contains(&r#"GRANT SELECT ("id") ON "hr"."profile" TO rootcx_app_executor"#.to_string()), "{plan:#?}");
        assert!(!plan.iter().any(|s| s.starts_with("GRANT SELECT ON")), "{plan:#?}");
    }

    #[test]
    fn a_table_without_sensitive_columns_is_granted_whole() {
        let plan = table_rls("hr", "profile", &facts(&["id"], None), None, &[], false).unwrap();
        assert!(plan.contains(&r#"GRANT SELECT ON "hr"."profile" TO rootcx_app_executor"#.to_string()), "{plan:#?}");
    }

    /// Removing an ownership declaration must remove the confinement rather than
    /// strand it, so the `_own` twins are dropped and not recreated.
    #[test]
    fn dropping_ownership_drops_the_row_scoped_twins() {
        let plan = table_rls("hr", "profile", &facts(&["id"], None), None, &[], false).unwrap();
        for (name, ..) in RLS_POLICIES {
            assert!(plan.contains(&format!(r#"DROP POLICY IF EXISTS {name}_own ON "hr"."profile""#)), "{name}");
            assert!(!plan.iter().any(|s| s.starts_with(&format!("CREATE POLICY {name}_own "))), "{name}");
        }
    }

    /// The caller's id is cast to the column's type, never the column to text, or
    /// a confined read stops being indexable.
    #[test]
    fn ownership_casts_the_caller_not_the_column() {
        let plan = table_rls("hr", "profile", &facts(&["id"], Some(direct("owner_id", "uuid"))), None, &[], false).unwrap();
        let own = plan.iter().find(|s| s.starts_with("CREATE POLICY rootcx_rls_select_own ")).expect("policy");
        assert!(own.contains(r#""owner_id" = (SELECT nullif(current_setting('rootcx.user_id', true), ''))::uuid"#), "{own}");
    }

    /// Every resolver a policy names is created before the policy that names it.
    #[test]
    fn resolvers_precede_the_policies_naming_them() {
        let owner = OwnerFacts {
            root_column: "owner_id".into(),
            root_type: "uuid".into(),
            links: vec![LinkFacts {
                parent: "project".into(), link: "project_id".into(),
                resolver: "rootcx_own.hr.project".into(), pk: "id".into(), pk_type: "uuid".into(),
            }],
        };
        let plan = table_rls("hr", "task", &facts(&["id", "project_id"], Some(owner)), None, &[], false).unwrap();
        let created = plan.iter().position(|s| s.starts_with("CREATE OR REPLACE FUNCTION")).expect("resolver");
        let named = plan.iter().position(|s| s.contains("rootcx_own.hr.project")
            && s.starts_with("CREATE POLICY")).expect("policy");
        assert!(created < named, "{plan:#?}");
    }

    /// An approved reader may lock a row, never write it.
    #[test]
    fn the_action_lock_never_passes_a_write_check() {
        let plan = table_rls("hr", "profile", &facts(&["id"], None), None, &[], true).unwrap();
        let lock = plan.iter().find(|s| s.starts_with("CREATE POLICY rootcx_rls_action_lock ")).expect("lock");
        assert!(lock.contains("FOR UPDATE"), "{lock}");
        assert!(lock.contains("WITH CHECK (FALSE)"), "{lock}");
        let plan = table_rls("hr", "profile", &facts(&["id"], None), None, &[], false).unwrap();
        assert!(!plan.iter().any(|s| s.starts_with("CREATE POLICY rootcx_rls_action_lock ")), "{plan:#?}");
    }

    /// The ceiling is RESTRICTIVE, so a provider's own permissive policy cannot
    /// widen a public execution, and no write is ever reachable through one.
    #[test]
    fn the_publication_ceiling_is_restrictive_and_denies_writes() {
        let plan = table_rls("hr", "profile", &facts(&["id"], None), None, &[], false).unwrap();
        for (name, _, action, _) in RLS_POLICIES {
            let ceiling = plan.iter()
                .find(|s| s.starts_with(&format!("CREATE POLICY {name}_publication_ceiling ")))
                .unwrap_or_else(|| panic!("{name}"));
            assert!(ceiling.contains("AS RESTRICTIVE"), "{ceiling}");
            if action != "read" {
                assert!(ceiling.contains("(FALSE)"), "{ceiling}");
            }
        }
    }
}
