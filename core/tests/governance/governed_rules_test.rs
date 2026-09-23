//! Governed declarative rules (ADR 0010) against a real Core and PostgreSQL:
//! legacy adoption without rebuilds, gap-free replacement under concurrent
//! writers, preflight that changes nothing, leftover cleanup and error mapping.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::harness::TestRuntime;
use reqwest::StatusCode;
use serde_json::{Value, json};

fn fnv(s: &str) -> String {
    let mut h = 0xcbf29ce484222325u64;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

/// The tag an older Core wrote for a declared or enum check.
fn legacy_check_tag(expr: &str) -> String {
    fnv(&expr.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn lines(app: &str, checks: Value, indexes: Value) -> Value {
    json!({
        "appId": app, "name": app, "version": "1.0.0",
        "dataContract": [{
            "entityName": "lines",
            "fields": [
                {"name": "qty", "type": "decimal", "precision": 12, "scale": 2},
                {"name": "status", "type": "text", "enum_values": ["open", "closed"]},
                {"name": "ref", "type": "text"}
            ],
            "checks": checks,
            "indexes": indexes
        }]
    })
}

async fn constraint(rt: &TestRuntime, app: &str, name: &str) -> Option<(u32, Option<String>)> {
    sqlx::query_as::<_, (sqlx::postgres::types::Oid, Option<String>)>(
        "SELECT con.oid, obj_description(con.oid, 'pg_constraint') FROM pg_constraint con
         JOIN pg_class t ON t.oid = con.conrelid JOIN pg_namespace n ON n.oid = t.relnamespace
         WHERE n.nspname = $1 AND con.conname = $2",
    ).bind(app).bind(name).fetch_optional(rt.pool()).await.unwrap().map(|(oid, c)| (oid.0, c))
}

async fn index(rt: &TestRuntime, app: &str, name: &str) -> Option<(u32, Option<String>)> {
    sqlx::query_as::<_, (sqlx::postgres::types::Oid, Option<String>)>(
        "SELECT c.oid, obj_description(c.oid, 'pg_class') FROM pg_class c
         JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = $1 AND c.relname = $2",
    ).bind(app).bind(name).fetch_optional(rt.pool()).await.unwrap().map(|(oid, c)| (oid.0, c))
}

async fn insert(rt: &TestRuntime, app: &str, values: &str) -> Result<(), String> {
    sqlx::raw_sql(&format!("INSERT INTO {app}.lines (qty, status, ref) VALUES {values}"))
        .execute(rt.pool()).await.map(|_| ())
        .map_err(|e| e.as_database_error().and_then(|d| d.code()).map(|c| c.to_string()).unwrap_or_else(|| e.to_string()))
}

/// Objects an older Core compiled from raw SQL are adopted by rewriting their
/// tag: no rebuild, no scan, same OIDs, and the rules stay enforced.
#[tokio::test]
async fn legacy_rules_are_adopted_by_retag() {
    let rt = TestRuntime::boot().await;
    let app = "rules_legacy";
    let check = "qty > 0 AND scale(qty) <= 2";
    let predicate = "status <> 'closed'";
    let full = lines(app, json!([{"name": "ck_lines_qty", "expr": check}]),
        json!([{"name": "uq_lines_ref", "columns": ["ref"], "unique": true, "where": predicate}]));
    rt.install_manifest(&lines(app, json!([]), json!([]))).await;

    // Recreate exactly what an older Core produced: raw SQL, FNV tags, and the
    // declarations in the stored manifest.
    let enum_expr = r#""status" IN ('open', 'closed')"#;
    let index_tag = fnv(&format!("u|btree|ref,|{predicate}|"));
    sqlx::raw_sql(&format!(r#"
        ALTER TABLE {app}.lines DROP CONSTRAINT chk_lines_status;
        ALTER TABLE {app}.lines ADD CONSTRAINT chk_lines_status CHECK ({enum_expr});
        COMMENT ON CONSTRAINT chk_lines_status ON {app}.lines IS 'rootcx:chk:{}';
        ALTER TABLE {app}.lines ADD CONSTRAINT ck_lines_qty CHECK ({check});
        COMMENT ON CONSTRAINT ck_lines_qty ON {app}.lines IS 'rootcx:chk:{}';
        CREATE UNIQUE INDEX uq_lines_ref ON {app}.lines USING btree ("ref") WHERE {predicate};
        COMMENT ON INDEX {app}.uq_lines_ref IS 'rootcx:idx:{index_tag}';
    "#, legacy_check_tag(enum_expr), legacy_check_tag(check))).execute(rt.pool()).await.unwrap();
    sqlx::query("UPDATE rootcx_system.apps SET manifest = $2 WHERE id = $1")
        .bind(app).bind(&full).execute(rt.pool()).await.unwrap();
    insert(&rt, app, "(1.5, 'open', 'a'), (2, 'closed', 'a')").await.unwrap();

    let before = [
        constraint(&rt, app, "chk_lines_status").await.unwrap().0,
        constraint(&rt, app, "ck_lines_qty").await.unwrap().0,
        index(&rt, app, "uq_lines_ref").await.unwrap().0,
    ];
    rt.install_manifest(&full).await;
    let after = [
        constraint(&rt, app, "chk_lines_status").await.unwrap(),
        constraint(&rt, app, "ck_lines_qty").await.unwrap(),
        index(&rt, app, "uq_lines_ref").await.unwrap(),
    ];
    for (oid, (new_oid, comment)) in before.iter().zip(&after) {
        assert_eq!(oid, new_oid, "an adopted object is never rebuilt");
        let comment = comment.as_deref().unwrap();
        assert!(comment.starts_with("rootcx:chk:r1-") || comment.starts_with("rootcx:idx:r1-"), "{comment}");
    }
    assert_eq!(insert(&rt, app, "(0, 'open', 'b')").await, Err("23514".into()));
    assert_eq!(insert(&rt, app, "(1, 'draft', 'b')").await, Err("23514".into()));
    assert_eq!(insert(&rt, app, "(1, 'open', 'a')").await, Err("23505".into()));

    // A second install keeps everything: tags now match.
    rt.install_manifest(&full).await;
    assert_eq!(constraint(&rt, app, "ck_lines_qty").await.unwrap(), after[1]);
    rt.shutdown().await;
}

/// Replacing a rule never opens a window in which neither version is
/// enforced: a writer hammering the table during reinstalls never lands a row
/// both versions forbid.
#[tokio::test]
async fn replacing_a_rule_keeps_it_enforced_under_concurrent_writes() {
    let rt = TestRuntime::boot().await;
    let app = "rules_swap";
    let with = |expr: &str, predicate: &str| lines(app, json!([{"name": "ck_lines_qty", "expr": expr}]),
        json!([{"name": "ix_lines_ref", "columns": ["ref"], "where": predicate}]));
    rt.install_manifest(&with("qty >= 0", "qty > 1")).await;
    insert(&rt, app, "(3, 'open', 'seed')").await.unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let leaked = Arc::new(AtomicUsize::new(0));
    let written = Arc::new(AtomicUsize::new(0));
    let writer = {
        let (pool, stop, leaked, written) = (rt.pool().clone(), stop.clone(), leaked.clone(), written.clone());
        tokio::spawn(async move {
            while !stop.load(Ordering::Relaxed) {
                let bad = sqlx::raw_sql(&format!("INSERT INTO {app}.lines (qty, ref) VALUES (-1, 'bad')")).execute(&pool).await;
                if bad.is_ok() {
                    leaked.fetch_add(1, Ordering::Relaxed);
                }
                if sqlx::raw_sql(&format!("INSERT INTO {app}.lines (qty, ref) VALUES (5, 'ok')")).execute(&pool).await.is_ok() {
                    written.fetch_add(1, Ordering::Relaxed);
                }
            }
        })
    };
    for (expr, predicate) in [("qty > 0", "qty > 2"), ("qty >= 0.5", "qty > 1"), ("qty > 0", "qty > 3")] {
        rt.install_manifest(&with(expr, predicate)).await;
    }
    stop.store(true, Ordering::Relaxed);
    writer.await.unwrap();

    assert_eq!(leaked.load(Ordering::Relaxed), 0, "a row both rules forbid was accepted");
    assert!(written.load(Ordering::Relaxed) > 0, "the writer kept writing during installs");
    assert_eq!(insert(&rt, app, "(0, 'open', 'x')").await, Err("23514".into()), "the new rule is enforced");
    let (_, comment) = constraint(&rt, app, "ck_lines_qty").await.unwrap();
    assert!(comment.unwrap().starts_with("rootcx:chk:r1-"));
    let leftovers: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM pg_constraint WHERE conname LIKE 'rootcx_next_%')
              + (SELECT count(*) FROM pg_class WHERE relname LIKE 'rootcx_next_%')",
    ).fetch_one(rt.pool()).await.unwrap();
    assert_eq!(leftovers, 0);
    rt.shutdown().await;
}

/// Existing rows that violate a new or changed rule abort the install before
/// any constraint or index changes.
#[tokio::test]
async fn violating_rows_abort_the_install_and_change_nothing() {
    let rt = TestRuntime::boot().await;
    let app = "rules_preflight";
    let original = lines(app, json!([{"name": "ck_lines_qty", "expr": "qty >= 0"}]),
        json!([{"name": "ix_lines_ref", "columns": ["ref"]}]));
    rt.install_manifest(&original).await;
    insert(&rt, app, "(0, 'open', 'dup'), (0, 'open', 'dup'), (1, 'open', 'x')").await.unwrap();
    let before = (
        constraint(&rt, app, "ck_lines_qty").await,
        index(&rt, app, "ix_lines_ref").await,
    );

    let tightened = lines(app, json!([
        {"name": "ck_lines_qty", "expr": "qty > 0"},
        {"name": "ck_lines_ref", "expr": "ref IS NOT NULL"}
    ]), json!([]));
    let (status, body) = rt.post_json("/api/v1/apps", &tightened).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let message = body.to_string();
    assert!(message.contains("rule 'ck_lines_qty' on 'lines' is violated by 2 existing rows") && message.contains("nothing was changed"), "{message}");
    assert_eq!(before, (constraint(&rt, app, "ck_lines_qty").await, index(&rt, app, "ix_lines_ref").await),
        "the old rule and the index to drop are untouched");
    assert!(constraint(&rt, app, "ck_lines_ref").await.is_none(), "no new rule was created");
    let (active, stored): (bool, Value) = sqlx::query_as(
        "SELECT i.active, a.manifest FROM rootcx_system.apps a
         JOIN rootcx_system.app_installations i ON i.app_id = a.id WHERE a.id = $1 ORDER BY i.active DESC LIMIT 1",
    ).bind(app).fetch_one(rt.pool()).await.unwrap();
    assert!(active, "a refused deploy leaves the running installation active");
    assert_eq!(stored["dataContract"][0]["checks"][0]["expr"], "qty >= 0", "the stored manifest is unchanged");

    let unique = lines(app, json!([{"name": "ck_lines_qty", "expr": "qty >= 0"}]),
        json!([{"name": "ix_lines_ref", "columns": ["ref"]}, {"name": "uq_lines_ref", "columns": ["ref"], "unique": true}]));
    let (status, body) = rt.post_json("/api/v1/apps", &unique).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.to_string().contains("unique index 'uq_lines_ref' on 'lines' would reject 1 group"), "{body}");
    assert!(index(&rt, app, "uq_lines_ref").await.is_none());
    rt.shutdown().await;
}

/// An interrupted install can leave an unvalidated twin or a stray index; the
/// next install removes them before anything else.
#[tokio::test]
async fn leftovers_of_an_interrupted_install_are_cleaned() {
    let rt = TestRuntime::boot().await;
    let app = "rules_leftovers";
    let manifest = lines(app, json!([{"name": "ck_lines_qty", "expr": "qty >= 0"}]), json!([]));
    rt.install_manifest(&manifest).await;
    sqlx::raw_sql(&format!(r#"
        ALTER TABLE {app}.lines ADD CONSTRAINT rootcx_next_0123456789abcdef CHECK (qty > 100) NOT VALID;
        COMMENT ON CONSTRAINT rootcx_next_0123456789abcdef ON {app}.lines IS 'rootcx:chk:pending-r1-0000000000000000';
        CREATE INDEX rootcx_next_fedcba9876543210 ON {app}.lines (ref);
    "#)).execute(rt.pool()).await.unwrap();
    rt.install_manifest(&manifest).await;
    assert!(constraint(&rt, app, "rootcx_next_0123456789abcdef").await.is_none());
    assert!(index(&rt, app, "rootcx_next_fedcba9876543210").await.is_none());
    insert(&rt, app, "(5, 'open', 'a')").await.expect("the stale twin no longer rejects writes");
    rt.shutdown().await;
}

/// A rejected write names the rule, never the row: PostgreSQL's DETAIL, which
/// carries values, stays inside Core.
#[tokio::test]
async fn rule_violations_are_422_and_duplicates_409_without_values() {
    let rt = TestRuntime::boot().await;
    let app = "rules_errors";
    rt.install_manifest(&lines(app, json!([{"name": "ck_lines_qty", "expr": "qty > 0"}]),
        json!([{"name": "uq_lines_ref", "columns": ["ref"], "unique": true}]))).await;
    let path = format!("/api/v1/apps/{app}/collections/lines");

    let (status, body) = rt.post_json(&path, &json!({"qty": "0", "ref": "secret-ref-1"})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body, json!({"error": "rule_violation", "entity": "lines", "rule": "ck_lines_qty"}));

    let (status, body) = rt.post_json(&path, &json!({"qty": "1", "ref": "secret-ref-2"})).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, body) = rt.post_json(&path, &json!({"qty": "2", "ref": "secret-ref-2"})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body, json!({"error": "unique_violation", "entity": "lines", "index": "uq_lines_ref"}));
    assert!(!body.to_string().contains("secret-ref"), "no value leaks: {body}");
    rt.shutdown().await;
}

async fn trigram_namespace(rt: &TestRuntime) -> Option<String> {
    sqlx::query_scalar("SELECT n.nspname::text FROM pg_extension e JOIN pg_namespace n ON n.oid = e.extnamespace WHERE e.extname = 'pg_trgm'")
        .fetch_optional(rt.pool()).await.unwrap()
}

fn with_trigram(app: &str, index: Value) -> Value {
    lines(app, json!([]), json!([index]))
}

/// A fresh installation gets pg_trgm in the Core-owned schema, never in an app
/// schema another app's index would then depend on.
#[tokio::test]
async fn trigram_indexes_install_pg_trgm_in_the_core_schema() {
    let rt = TestRuntime::boot().await;
    assert_eq!(trigram_namespace(&rt).await, None, "fixture starts without pg_trgm");
    let app = "rules_trgm_fresh";
    rt.install_manifest(&with_trigram(app, json!({"name": "ix_lines_ref_trgm", "using": "trigram", "columns": ["ref"]}))).await;
    assert_eq!(trigram_namespace(&rt).await.as_deref(), Some("rootcx_ext"));
    let definition: String = sqlx::query_scalar("SELECT pg_get_indexdef('rules_trgm_fresh.ix_lines_ref_trgm'::regclass)")
        .fetch_one(rt.pool()).await.unwrap();
    assert!(definition.contains("USING gin (ref rootcx_ext.gin_trgm_ops)"), "{definition}");
    rt.shutdown().await;
}

/// An index an older Core built with a raw `ops` is adopted by catalog
/// structure; uninstalling the app whose schema holds pg_trgm moves the
/// extension out first, so another app's trigram index survives.
#[tokio::test]
async fn legacy_trigram_indexes_adopt_and_survive_the_owner_schema_uninstall() {
    let rt = TestRuntime::boot().await;
    let (owner, other) = ("rules_trgm_owner", "rules_trgm_other");
    let legacy = json!({"name": "ix_lines_ref_trgm", "using": "gin", "columns": [{"column": "ref", "ops": format!("{owner}.gin_trgm_ops")}]});
    let declared = json!({"name": "ix_lines_ref_trgm", "using": "trigram", "columns": ["ref"]});
    rt.install_manifest(&lines(owner, json!([]), json!([]))).await;
    sqlx::raw_sql(&format!(r#"
        CREATE EXTENSION pg_trgm WITH SCHEMA {owner};
        CREATE INDEX ix_lines_ref_trgm ON {owner}.lines USING gin ("ref" {owner}.gin_trgm_ops);
        COMMENT ON INDEX {owner}.ix_lines_ref_trgm IS 'rootcx:idx:{}';
    "#, fnv(&format!("_|gin|ref:::::{owner}.gin_trgm_ops:,||")))).execute(rt.pool()).await.unwrap();
    sqlx::query("UPDATE rootcx_system.apps SET manifest = $2 WHERE id = $1")
        .bind(owner).bind(with_trigram(owner, legacy)).execute(rt.pool()).await.unwrap();

    let before = index(&rt, owner, "ix_lines_ref_trgm").await.unwrap().0;
    rt.install_manifest(&with_trigram(owner, declared.clone())).await;
    let (after, comment) = index(&rt, owner, "ix_lines_ref_trgm").await.unwrap();
    assert_eq!(before, after, "the legacy trigram index is adopted, not rebuilt");
    assert!(comment.unwrap().starts_with("rootcx:idx:r1-"));
    assert_eq!(trigram_namespace(&rt).await.as_deref(), Some(owner), "installed extensions are not moved by installs");

    rt.install_manifest(&with_trigram(other, declared)).await;
    let other_index = index(&rt, other, "ix_lines_ref_trgm").await.unwrap().0;
    let (status, body) = rt.delete_json(&format!("/api/v1/apps/{owner}")).await;
    assert!(status.is_success(), "{status} {body}");
    assert_eq!(trigram_namespace(&rt).await.as_deref(), Some("rootcx_ext"), "moved before the schema was dropped");
    assert_eq!(index(&rt, other, "ix_lines_ref_trgm").await.map(|i| i.0), Some(other_index), "the other app's index survives");
    let used: bool = sqlx::query_scalar(&format!(
        "SELECT count(*) = 0 FROM {other}.lines WHERE ref OPERATOR(rootcx_ext.%) 'abc'"
    )).fetch_one(rt.pool()).await.unwrap();
    assert!(used, "the operator class still works from its new schema");
    rt.shutdown().await;
}

#[tokio::test]
async fn core_and_extension_schemas_are_not_app_ids() {
    let rt = TestRuntime::boot().await;
    for app in ["rootcx_ext", "rootcx_system", "pgmq", "cron", "public", "pg_temp_x"] {
        let (status, body) = rt.post_json("/api/v1/apps", &lines(app, json!([]), json!([]))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{app}: {body}");
    }
    rt.shutdown().await;
}

fn quote(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

/// How an older Core hashed an index declaration (`schema_sync::index_spec_hash`).
fn legacy_index_tag(index: &Value) -> String {
    let mut s = String::from(if index["unique"].as_bool().unwrap_or(false) { "u|" } else { "_|" });
    s.push_str(index["using"].as_str().unwrap_or("btree"));
    s.push('|');
    for column in index["columns"].as_array().unwrap() {
        match column.as_str() {
            Some(name) => s.push_str(name),
            None => for key in ["column", "expr", "sort", "nulls", "ops"] {
                s.push_str(column[key].as_str().unwrap_or(""));
                s.push(':');
            },
        }
        s.push(',');
    }
    s.push('|');
    s.push_str(index["where"].as_str().unwrap_or(""));
    s.push('|');
    fnv(&s)
}

/// What an older Core executed for an index: the raw declaration.
fn legacy_index_sql(schema: &str, table: &str, index: &Value) -> String {
    let columns: Vec<String> = index["columns"].as_array().unwrap().iter().map(|c| match c.as_str() {
        Some(name) => quote(name),
        None => {
            let mut element = match (c["column"].as_str(), c["expr"].as_str()) {
                (Some(column), _) => quote(column),
                (None, Some(expr)) => format!("({expr})"),
                _ => unreachable!(),
            };
            if let Some(ops) = c["ops"].as_str() {
                element.push_str(&format!(" {ops}"));
            }
            if let Some(sort) = c["sort"].as_str() {
                element.push_str(&format!(" {}", sort.to_uppercase()));
            }
            if let Some(nulls) = c["nulls"].as_str() {
                element.push_str(&format!(" NULLS {}", nulls.to_uppercase()));
            }
            element
        }
    }).collect();
    let mut sql = format!(
        "CREATE {}INDEX {} ON {}.{} USING {} ({})",
        if index["unique"].as_bool().unwrap_or(false) { "UNIQUE " } else { "" },
        quote(index["name"].as_str().unwrap()), quote(schema), quote(table),
        index["using"].as_str().unwrap_or("btree"), columns.join(", "),
    );
    if let Some(predicate) = index["where"].as_str() {
        sql.push_str(&format!(" WHERE {predicate}"));
    }
    sql
}

/// A real application's manifest, kept out of the repository
/// (`core/tests/fixtures/private/corpus_manifest.json`). Its constraints and
/// indexes are recreated exactly as an older Core built them from raw SQL; the
/// new Core must adopt every one without rebuilding any.
#[tokio::test]
async fn private_corpus_adopts_every_rule_without_rebuilding() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/private/corpus_manifest.json");
    let Ok(raw) = std::fs::read_to_string(path) else { return };
    let original: Value = serde_json::from_str(&raw).unwrap();
    let app = original["appId"].as_str().unwrap().to_string();
    // The two edits the authors need: trigram indexes, no misspelled keys.
    let mut declared = original.clone();
    for entity in declared["dataContract"].as_array_mut().unwrap() {
        for field in entity["fields"].as_array_mut().unwrap() {
            field.as_object_mut().unwrap().remove("onDelete");
        }
        for index in entity.get_mut("indexes").and_then(|i| i.as_array_mut()).into_iter().flatten() {
            let trigram = index["columns"].as_array().unwrap().iter()
                .any(|c| c["ops"].as_str().is_some_and(|o| o.ends_with("gin_trgm_ops")));
            if trigram {
                let names: Vec<Value> = index["columns"].as_array().unwrap().iter().map(|c| c["column"].clone()).collect();
                index["columns"] = json!(names);
                index["using"] = json!("trigram");
            }
        }
    }
    for key in ["crons", "actions"] {
        declared.as_object_mut().unwrap().remove(key);
    }

    let rt = TestRuntime::boot().await;
    rt.install_manifest(&declared).await;
    let trigram = trigram_namespace(&rt).await.unwrap();

    // Rebuild every rule the way an older Core did.
    let mut legacy = Vec::new();
    for entity in original["dataContract"].as_array().unwrap() {
        let table = entity["entityName"].as_str().unwrap();
        let fq = format!("{}.{}", quote(&app), quote(table));
        for check in entity["checks"].as_array().into_iter().flatten() {
            let (name, expr) = (check["name"].as_str().unwrap(), check["expr"].as_str().unwrap());
            legacy.push(format!("ALTER TABLE {fq} DROP CONSTRAINT {}", quote(name)));
            legacy.push(format!("ALTER TABLE {fq} ADD CONSTRAINT {} CHECK ({expr})", quote(name)));
            legacy.push(format!("COMMENT ON CONSTRAINT {} ON {fq} IS 'rootcx:chk:{}'", quote(name), legacy_check_tag(expr)));
        }
        for field in entity["fields"].as_array().unwrap() {
            let Some(values) = field["enum_values"].as_array().filter(|v| !v.is_empty()) else { continue };
            let name = format!("chk_{table}_{}", field["name"].as_str().unwrap());
            if name.len() > 63 { continue }
            let list = values.iter().map(|v| format!("'{}'", v.as_str().unwrap().replace('\'', "''"))).collect::<Vec<_>>().join(", ");
            let column = quote(field["name"].as_str().unwrap());
            let expr = if field["type"] == "[text]" { format!("{column} <@ ARRAY[{list}]::TEXT[]") } else { format!("{column} IN ({list})") };
            legacy.push(format!("ALTER TABLE {fq} DROP CONSTRAINT {}", quote(&name)));
            legacy.push(format!("ALTER TABLE {fq} ADD CONSTRAINT {} CHECK ({expr})", quote(&name)));
            legacy.push(format!("COMMENT ON CONSTRAINT {} ON {fq} IS 'rootcx:chk:{}'", quote(&name), legacy_check_tag(&expr)));
        }
        for index in entity["indexes"].as_array().into_iter().flatten() {
            let name = index["name"].as_str().unwrap();
            let mut raw = index.clone();
            for column in raw["columns"].as_array_mut().unwrap() {
                if let Some(ops) = column.get_mut("ops") {
                    *ops = json!(format!("{}.gin_trgm_ops", quote(&trigram)));
                }
            }
            legacy.push(format!("DROP INDEX {}.{}", quote(&app), quote(name)));
            legacy.push(legacy_index_sql(&app, table, &raw));
            legacy.push(format!("COMMENT ON INDEX {}.{} IS 'rootcx:idx:{}'", quote(&app), quote(name), legacy_index_tag(index)));
        }
    }
    let mut tx = rt.pool().begin().await.unwrap();
    for statement in &legacy {
        sqlx::query(statement).execute(&mut *tx).await.unwrap_or_else(|e| panic!("{statement}: {e}"));
    }
    tx.commit().await.unwrap();
    sqlx::query("UPDATE rootcx_system.apps SET manifest = $2 WHERE id = $1")
        .bind(&app).bind(&original).execute(rt.pool()).await.unwrap();

    let objects = "SELECT kind, name, oid::bigint, comment FROM (
          SELECT 'check' kind, con.conname::text name, con.oid, obj_description(con.oid, 'pg_constraint') comment
          FROM pg_constraint con JOIN pg_namespace n ON n.oid = con.connamespace WHERE n.nspname = $1 AND con.contype = 'c'
          UNION ALL
          SELECT 'index', c.relname::text, c.oid, obj_description(c.oid, 'pg_class')
          FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = $1 AND c.relkind = 'i'
        ) o WHERE comment LIKE 'rootcx:%' ORDER BY kind, name";
    let before: Vec<(String, String, i64, String)> = sqlx::query_as(objects).bind(&app).fetch_all(rt.pool()).await.unwrap();
    assert!(before.iter().all(|(_, _, _, c)| !c.contains(":r1-")), "every rule starts with a legacy tag");

    rt.install_manifest(&declared).await;
    let after: Vec<(String, String, i64, String)> = sqlx::query_as(objects).bind(&app).fetch_all(rt.pool()).await.unwrap();
    let rebuilt: Vec<&String> = before.iter().zip(&after)
        .filter(|((k1, n1, o1, _), (k2, n2, o2, _))| (k1, n1, o1) != (k2, n2, o2))
        .map(|((_, name, _, _), _)| name).collect();
    assert_eq!(before.len(), after.len(), "no rule was lost or added");
    assert!(rebuilt.is_empty(), "rebuilt instead of adopted: {rebuilt:?}");
    assert!(after.iter().all(|(_, _, _, c)| c.contains(":r1-")), "every rule is now governed");
    let entities = original["dataContract"].as_array().unwrap();
    let count = |f: &dyn Fn(&Value) -> usize| entities.iter().map(f).sum::<usize>();
    let declared_checks = count(&|e| e["checks"].as_array().map_or(0, Vec::len));
    let enum_checks = count(&|e| e["fields"].as_array().unwrap().iter()
        .filter(|f| f["enum_values"].as_array().is_some_and(|v| !v.is_empty())).count());
    let declared_indexes = count(&|e| e["indexes"].as_array().map_or(0, Vec::len));
    assert_eq!(after.iter().filter(|o| o.0 == "check").count(), declared_checks + enum_checks);
    assert_eq!(after.iter().filter(|o| o.0 == "index").count(), declared_indexes);
    rt.shutdown().await;
}
