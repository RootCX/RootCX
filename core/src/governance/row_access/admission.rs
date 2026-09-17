//! Admission of app SQL artifacts, not recovery of a compromised database.
//!
//! Core/system catalogs and Core trigger functions must come from a trusted
//! database. Historical owner migrations could modify objects outside the app:
//! restore/rebuild a trusted database if that provenance cannot be established.
//! Call before touching app relations (including the migration ledger). Hold the
//! lifecycle lock through inspection and reconciliation. Recognized Core policy
//! names are reserved slots, NOT proof of their contents: the caller must replace
//! ALL policies transactionally before enabling runtime access.

use std::collections::HashSet;

use rootcx_types::{AppManifest, IndexColumn};
use sqlx::{PgConnection, Row};

use crate::RuntimeError;

fn refused(schema: &str, object: &str, reason: &str) -> RuntimeError {
    RuntimeError::Invalid(format!(
        "SQL admission for '{schema}': {object}: {reason}. \
         Remove or replace this artifact through an independently controlled administrator \
         migration before retrying; app SQL is not executed to repair it"
    ))
}

pub(super) fn validate_manifest(manifest: &AppManifest) -> Result<(), RuntimeError> {
    for entity in &manifest.data_contract {
        let reject = |reason: &str| refused(&manifest.app_id, &entity.entity_name, reason);
        if !entity.checks.is_empty() {
            return Err(reject(
                "raw CHECK expressions are not supported; use field enums",
            ));
        }
        for index in &entity.indexes {
            if index.where_clause.is_some() || !index.with.is_empty() {
                return Err(reject(
                    "raw index predicates and storage parameters are not supported",
                ));
            }
            for column in &index.columns {
                let name = match column {
                    IndexColumn::Name(name) => name,
                    IndexColumn::Spec(spec) => {
                        if spec.expr.is_some() || spec.ops.is_some() {
                            return Err(reject(
                                "index expressions and operator classes are not supported",
                            ));
                        }
                        spec.column
                            .as_ref()
                            .ok_or_else(|| reject("index column is missing"))?
                    }
                };
                if !crate::manifest::is_system_field(name)
                    && !entity.fields.iter().any(|field| field.name == *name)
                {
                    return Err(reject("index column is not a declared or system field"));
                }
            }
            // Reuse the renderer's closed vocabularies and shape checks, before
            // DDL rather than after creating tables or dropping old indexes.
            crate::schema_sync::generate_create_index(&manifest.app_id, &entity.entity_name, index)
                .map_err(|error| reject(&error))?;
        }
    }
    Ok(())
}

const SAFE_TYPES: &[&str] = &[
    "bool",
    "int2",
    "int4",
    "int8",
    "float4",
    "float8",
    "numeric",
    "text",
    "varchar",
    "bpchar",
    "uuid",
    "date",
    "timestamp",
    "timestamptz",
    "json",
    "jsonb",
    "_text",
    "_float8",
];

/// pg_node_tree is PostgreSQL's own serialization, not SQL supplied by the app.
/// Only accept known node kinds. CONST payloads are serialized as byte numbers,
/// so they cannot smuggle braces or field names into this scanner.
fn node_kinds(tree: &str) -> impl Iterator<Item = &str> {
    tree.split('{').skip(1).map(|node| {
        node.split_whitespace()
            .next()
            .unwrap_or("")
            .trim_end_matches('}')
    })
}

fn oid_fields<'a>(tree: &'a str, key: &'a str) -> impl Iterator<Item = Option<u32>> + 'a {
    tree.split_whitespace()
        .zip(tree.split_whitespace().skip(1))
        .filter_map(move |(field, value)| {
            (field == key).then(|| value.trim_end_matches(['}', ')']).parse().ok())
        })
}

#[derive(Clone, Copy)]
enum Expression {
    Default,
    Check,
    Predicate,
}

fn safe_expression(
    tree: &str,
    kind: Expression,
    types: &HashSet<u32>,
    operators: &HashSet<u32>,
    defaults: &HashSet<u32>,
) -> bool {
    let allowed: &[&str] = match kind {
        Expression::Default => &["CONST", "FUNCEXPR", "RELABELTYPE"],
        Expression::Check => &[
            "CONST",
            "VAR",
            "OPEXPR",
            "SCALARARRAYOPEXPR",
            "ARRAYEXPR",
            "RELABELTYPE",
        ],
        // The share compiler emits exactly a scalar-column IS NULL. Do not
        // allow arbitrary boolean trees or function expressions here.
        Expression::Predicate => &["NULLTEST", "VAR"],
    };
    let nodes: Vec<_> = node_kinds(tree).collect();
    if nodes.is_empty() || nodes.iter().any(|node| !allowed.contains(node)) {
        return false;
    }
    if matches!(kind, Expression::Predicate)
        && (nodes != ["NULLTEST", "VAR"]
            || !tree.contains(":nulltesttype 0 ")
            || !tree.contains(":argisrow false "))
    {
        return false;
    }
    for key in [
        ":consttype",
        ":vartype",
        ":resulttype",
        ":funcresulttype",
        ":array_typeid",
        ":element_typeid",
    ] {
        if oid_fields(tree, key).any(|oid| oid.is_none_or(|oid| !types.contains(&oid))) {
            return false;
        }
    }
    if oid_fields(tree, ":opno").any(|oid| oid.is_none_or(|oid| !operators.contains(&oid))) {
        return false;
    }
    if oid_fields(tree, ":funcid").any(|oid| {
        !matches!(kind, Expression::Default) || oid.is_none_or(|oid| !defaults.contains(&oid))
    }) {
        return false;
    }
    true
}

/// Metadata only: never prepare, EXPLAIN, SELECT from, or evaluate an app object.
pub(super) async fn inspect(conn: &mut PgConnection, schema: &str) -> Result<(), RuntimeError> {
    // Fully qualify catalog relations/functions: a legacy search_path must not
    // redirect admission into an app-owned function or view.
    let bad: Option<(String, String)> = sqlx::query_as(
        "SELECT object, reason FROM (
           SELECT p.proname::text AS object, 'legacy routine'::text AS reason
             FROM pg_catalog.pg_proc p JOIN pg_catalog.pg_namespace n ON n.oid=p.pronamespace
            WHERE n.nspname=$1
           UNION ALL
           SELECT c.relname::text, 'view, foreign table, partition or unsupported relation'
             FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace
            WHERE n.nspname=$1 AND (c.relkind NOT IN ('r','i','S') OR c.relispartition)
           UNION ALL
           SELECT c.relname::text, 'rewrite rule'
             FROM pg_catalog.pg_rewrite r JOIN pg_catalog.pg_class c ON c.oid=r.ev_class
             JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1
           UNION ALL
           SELECT c.relname::text, 'inheritance or partition relationship'
             FROM pg_catalog.pg_inherits i
             JOIN pg_catalog.pg_class c ON c.oid=i.inhrelid OR c.oid=i.inhparent
             JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1
           UNION ALL
           SELECT t.typname::text, 'custom type or domain'
             FROM pg_catalog.pg_type t JOIN pg_catalog.pg_namespace n ON n.oid=t.typnamespace
            WHERE n.nspname=$1
              AND NOT (t.typtype='c' AND t.typrelid<>0)
              AND NOT EXISTS (
                SELECT 1 FROM pg_catalog.pg_type e
                 WHERE e.oid=t.typelem AND e.typtype='c' AND e.typrelid<>0 AND e.typarray=t.oid)
           UNION ALL
           SELECT o.oprname::text, 'custom operator'
             FROM pg_catalog.pg_operator o JOIN pg_catalog.pg_namespace n ON n.oid=o.oprnamespace
            WHERE n.nspname=$1
           UNION ALL
           SELECT o.opcname::text, 'custom operator class'
             FROM pg_catalog.pg_opclass o JOIN pg_catalog.pg_namespace n ON n.oid=o.opcnamespace
            WHERE n.nspname=$1
         ) artifacts ORDER BY object, reason LIMIT 1",
    )
    .bind(schema)
    .fetch_optional(&mut *conn)
    .await
    .map_err(RuntimeError::Schema)?;
    if let Some((object, reason)) = bad {
        return Err(refused(schema, &object, &reason));
    }

    let types: HashSet<u32> = sqlx::query_scalar::<_, sqlx::postgres::types::Oid>(
        "SELECT t.oid FROM pg_catalog.pg_type t
         JOIN pg_catalog.pg_namespace n ON n.oid=t.typnamespace
         WHERE n.nspname='pg_catalog' AND (
           t.typname=ANY($1) OR EXISTS (
             SELECT 1 FROM pg_catalog.pg_type element
             WHERE element.oid=t.typelem AND element.typarray=t.oid
               AND element.typnamespace=n.oid AND element.typname=ANY($1)))",
    )
    .bind(SAFE_TYPES)
    .fetch_all(&mut *conn)
    .await
    .map_err(RuntimeError::Schema)?
    .into_iter()
    .map(|oid| oid.0)
    .collect();
    let operators: HashSet<u32> = sqlx::query_scalar::<_, sqlx::postgres::types::Oid>(
        "SELECT o.oid FROM pg_catalog.pg_operator o
         JOIN pg_catalog.pg_namespace n ON n.oid=o.oprnamespace
         JOIN pg_catalog.pg_proc p ON p.oid=o.oprcode
         WHERE n.nspname='pg_catalog' AND p.pronamespace=n.oid
           AND o.oprname IN ('=', '<@')",
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(RuntimeError::Schema)?
    .into_iter()
    .map(|oid| oid.0)
    .collect();
    let defaults: HashSet<u32> = sqlx::query_scalar::<_, sqlx::postgres::types::Oid>(
        "SELECT p.oid FROM pg_catalog.pg_proc p
         JOIN pg_catalog.pg_namespace n ON n.oid=p.pronamespace
         WHERE n.nspname='pg_catalog' AND p.proname IN ('now','gen_random_uuid')
           AND p.pronargs=0",
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(RuntimeError::Schema)?
    .into_iter()
    .map(|oid| oid.0)
    .collect();

    let columns = sqlx::query(
        "SELECT c.relname, a.attname, a.atttypid, a.attgenerated::text,
                a.attcollation=0 OR EXISTS (
                  SELECT 1 FROM pg_catalog.pg_collation col
                  JOIN pg_catalog.pg_namespace cn ON cn.oid=col.collnamespace
                  WHERE col.oid=a.attcollation AND cn.nspname='pg_catalog'
                ) AS safe_collation,
                d.adbin::text AS tree
         FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace
         JOIN pg_catalog.pg_attribute a ON a.attrelid=c.oid
         LEFT JOIN pg_catalog.pg_attrdef d ON d.adrelid=c.oid AND d.adnum=a.attnum
         WHERE n.nspname=$1 AND c.relkind='r' AND a.attnum>0 AND NOT a.attisdropped",
    )
    .bind(schema)
    .fetch_all(&mut *conn)
    .await
    .map_err(RuntimeError::Schema)?;
    for row in columns {
        let object = format!(
            "{}.{}",
            row.get::<String, _>("relname"),
            row.get::<String, _>("attname")
        );
        let ty = row.get::<sqlx::postgres::types::Oid, _>("atttypid").0;
        let tree: Option<String> = row.get("tree");
        if !types.contains(&ty)
            || !row.get::<bool, _>("safe_collation")
            || !row.get::<String, _>("attgenerated").is_empty()
            || tree.as_deref().is_some_and(|tree| {
                !safe_expression(tree, Expression::Default, &types, &operators, &defaults)
            })
        {
            return Err(refused(
                schema,
                &object,
                "unsupported column type, generated expression or executable default",
            ));
        }
    }

    let checks: Vec<(String, String, String, Option<String>, bool)> = sqlx::query_as(
        "SELECT c.relname::text, k.conname::text, k.contype::text, k.conbin::text,
                k.confrelid=0 OR EXISTS (
                  SELECT 1 FROM pg_catalog.pg_class target
                  JOIN pg_catalog.pg_namespace tn ON tn.oid=target.relnamespace
                  WHERE target.oid=k.confrelid AND target.relkind='r'
                    AND (tn.nspname=$1 OR
                         (tn.nspname='rootcx_system' AND target.relname='users')))
         FROM pg_catalog.pg_constraint k JOIN pg_catalog.pg_class c ON c.oid=k.conrelid
         JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1",
    )
    .bind(schema)
    .fetch_all(&mut *conn)
    .await
    .map_err(RuntimeError::Schema)?;
    for (table, name, kind, tree, target_safe) in checks {
        let safe = match kind.as_str() {
            "p" | "u" | "n" => true,
            "f" => target_safe,
            "c" => tree.as_deref().is_some_and(|tree| {
                safe_expression(tree, Expression::Check, &types, &operators, &defaults)
            }),
            _ => false,
        };
        if !safe {
            return Err(refused(
                schema,
                &format!("{table}.{name}"),
                "unsupported constraint or executable CHECK",
            ));
        }
    }

    let indexes: Vec<(String, Option<String>, Option<String>, bool)> = sqlx::query_as(
        "SELECT ix.relname::text, i.indexprs::text, i.indpred::text,
                am.amname IN ('btree','hash','gist','gin','spgist','brin')
                AND NOT EXISTS (
                  SELECT 1 FROM pg_catalog.unnest(i.indclass) op(oid)
                  JOIN pg_catalog.pg_opclass oc ON oc.oid=op.oid
                  JOIN pg_catalog.pg_namespace ns ON ns.oid=oc.opcnamespace
                  WHERE ns.nspname<>'pg_catalog')
                AND NOT EXISTS (
                  SELECT 1 FROM pg_catalog.unnest(i.indclass) op(oid)
                  JOIN pg_catalog.pg_opclass oc ON oc.oid=op.oid
                  JOIN pg_catalog.pg_amproc ap ON ap.amprocfamily=oc.opcfamily
                  JOIN pg_catalog.pg_proc p ON p.oid=ap.amproc
                  JOIN pg_catalog.pg_namespace ns ON ns.oid=p.pronamespace
                  WHERE ns.nspname<>'pg_catalog')
                AND NOT EXISTS (
                  SELECT 1 FROM pg_catalog.unnest(i.indclass) op(oid)
                  JOIN pg_catalog.pg_opclass oc ON oc.oid=op.oid
                  JOIN pg_catalog.pg_amop ao ON ao.amopfamily=oc.opcfamily
                  JOIN pg_catalog.pg_operator o ON o.oid=ao.amopopr
                  JOIN pg_catalog.pg_namespace ns ON ns.oid=o.oprnamespace
                  JOIN pg_catalog.pg_proc p ON p.oid=o.oprcode
                  WHERE ns.nspname<>'pg_catalog' OR p.pronamespace<>ns.oid)
                AND NOT EXISTS (
                  SELECT 1 FROM pg_catalog.unnest(i.indcollation) co(oid)
                  JOIN pg_catalog.pg_collation col ON col.oid=co.oid
                  JOIN pg_catalog.pg_namespace ns ON ns.oid=col.collnamespace
                  WHERE ns.nspname<>'pg_catalog')
         FROM pg_catalog.pg_index i JOIN pg_catalog.pg_class c ON c.oid=i.indrelid
         JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace
         JOIN pg_catalog.pg_class ix ON ix.oid=i.indexrelid
         JOIN pg_catalog.pg_am am ON am.oid=ix.relam WHERE n.nspname=$1",
    )
    .bind(schema)
    .fetch_all(&mut *conn)
    .await
    .map_err(RuntimeError::Schema)?;
    for (name, expression, predicate, safe_classes) in indexes {
        if !safe_classes
            || expression.is_some()
            || predicate.as_deref().is_some_and(|tree| {
                !safe_expression(tree, Expression::Predicate, &types, &operators, &defaults)
            })
        {
            return Err(refused(
                schema,
                &name,
                "expression index, unsafe operator class/collation or predicate",
            ));
        }
    }

    let trigger: Option<(String, String)> = sqlx::query_as(
        "SELECT c.relname::text, t.tgname::text
         FROM pg_catalog.pg_trigger t JOIN pg_catalog.pg_class c ON c.oid=t.tgrelid
         JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace
         JOIN pg_catalog.pg_proc p ON p.oid=t.tgfoid
         JOIN pg_catalog.pg_namespace pn ON pn.oid=p.pronamespace
         LEFT JOIN pg_catalog.pg_constraint k ON k.oid=t.tgconstraint
         WHERE n.nspname=$1 AND (
           (t.tgisinternal AND k.contype='f' AND pn.nspname='pg_catalog'
            AND p.proname IN ('RI_FKey_check_ins','RI_FKey_check_upd',
              'RI_FKey_noaction_del','RI_FKey_noaction_upd','RI_FKey_restrict_del',
              'RI_FKey_restrict_upd','RI_FKey_cascade_del','RI_FKey_cascade_upd',
              'RI_FKey_setnull_del','RI_FKey_setnull_upd',
              'RI_FKey_setdefault_del','RI_FKey_setdefault_upd'))
           OR
           (NOT t.tgisinternal AND t.tgtype=29 AND t.tgenabled='O'
            AND t.tgnargs=0 AND pg_catalog.octet_length(t.tgargs)=0 AND t.tgqual IS NULL
            AND t.tgconstraint=0 AND NOT t.tgdeferrable AND NOT t.tginitdeferred
            AND t.tgattr::text='' AND t.tgoldtable IS NULL AND t.tgnewtable IS NULL
            AND pn.nspname='rootcx_system' AND p.pronargs=0
            AND ((p.proname='audit_trigger_fn' AND t.tgname=pg_catalog.left(
                 pg_catalog.regexp_replace('audit_' || pg_catalog.format('%I.%I',n.nspname,c.relname),
                                           '[^a-zA-Z0-9_]', '_', 'g'),63))
              OR (p.proname='hooks_trigger_fn' AND t.tgname=pg_catalog.left(
                 pg_catalog.regexp_replace('hooks_' || pg_catalog.format('%I.%I',n.nspname,c.relname),
                                           '[^a-zA-Z0-9_]', '_', 'g'),63))))
         ) IS NOT TRUE LIMIT 1",
    ).bind(schema).fetch_optional(&mut *conn).await.map_err(RuntimeError::Schema)?;
    if let Some((table, name)) = trigger {
        return Err(refused(
            schema,
            &format!("{table}.{name}"),
            "unrecognized trigger definition",
        ));
    }

    // These slots are unconditionally regenerated by the caller, never trusted.
    let policies: Vec<(String, String)> = sqlx::query_as(
        "SELECT c.relname::text, p.polname::text FROM pg_catalog.pg_policy p
         JOIN pg_catalog.pg_class c ON c.oid=p.polrelid
         JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1",
    )
    .bind(schema)
    .fetch_all(&mut *conn)
    .await
    .map_err(RuntimeError::Schema)?;
    for (table, name) in policies {
        let reserved = ["select", "insert", "update", "delete"]
            .iter()
            .any(|command| {
                ["", "_own", "_publication_ceiling"]
                    .iter()
                    .any(|suffix| name == format!("rootcx_rls_{command}{suffix}"))
            })
            || matches!(
                name.as_str(),
                "rootcx_rls_select_publication" | "rootcx_rls_select_shared" | "rootcx_rls_action_lock"
            );
        if !reserved {
            return Err(refused(
                schema,
                &format!("{table}.{name}"),
                "unexpected custom row-security policy",
            ));
        }
    }
    Ok(())
}
