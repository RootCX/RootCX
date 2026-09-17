//! Manifest and legacy-schema admission before DDL and migration-ledger reads.
//!
//! Mutation targets:
//! - Remove manifest refusal: raw-fragment requests succeed or create a schema.
//! - Move inspection after DDL: the pending-schema test finds the new relation.
//! - Trust object comments/names: forged CHECK/trigger fixtures are admitted.
//! - Skip routines/views/policies/defaults: the corresponding fixture succeeds.
//! - Inspect after reading schema_migrations: the ledger-view marker is written.
//! Fixture function bodies are inert until called; admission must never call them.

use crate::harness::TestRuntime;
use reqwest::StatusCode;
use serde_json::{Value, json};

fn manifest(app: &str) -> Value {
    json!({
        "appId": app, "name": app, "version": "1.0.0",
        "dataContract": [{
            "entityName": "items",
            "fields": [{"name": "status", "type": "text"}]
        }]
    })
}

#[tokio::test]
async fn raw_manifest_sql_is_refused_before_creating_a_schema() {
    let rt = TestRuntime::boot().await;
    let cases = [
        ("check", json!({"checks": [{"expr": "true"}]})),
        (
            "expression",
            json!({"indexes": [{"columns": [{"expr": "lower(status)"}]}]}),
        ),
        (
            "predicate",
            json!({"indexes": [{"columns": ["status"], "where": "true"}]}),
        ),
        (
            "operator",
            json!({"indexes": [{"columns": [{"column": "status", "ops": "text_ops"}]}]}),
        ),
        (
            "storage",
            json!({"indexes": [{"columns": ["status"], "with": {"fillfactor": "70"}}]}),
        ),
        (
            "empty_expression",
            json!({"indexes": [{"columns": [{"expr": ""}]}]}),
        ),
        ("empty_index", json!({"indexes": [{"columns": []}]})),
        (
            "unknown_column",
            json!({"indexes": [{"columns": ["undeclared"]}]}),
        ),
    ];
    for (name, patch) in cases {
        let app = format!("admission_{name}");
        let mut request = manifest(&app);
        request["dataContract"][0]
            .as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        let (status, body) = rt.post_json("/api/v1/apps", &request).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{name}: {body}");
        assert!(body.to_string().contains("SQL admission"), "{name}: {body}");
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_namespace WHERE nspname=$1)")
                .bind(&app)
                .fetch_one(rt.pool())
                .await
                .unwrap();
        assert!(!exists, "{name}: refusal must precede CREATE SCHEMA");
    }
    rt.shutdown().await;
}

#[tokio::test]
async fn generated_artifacts_literal_defaults_and_simple_indexes_survive_reinstall() {
    let rt = TestRuntime::boot().await;
    let app = "admission_safe";
    let literal = "pg_catalog.pg_read_file('/not/a/file')";
    let request = json!({
        "appId": app, "name": app, "version": "1.0.0",
        "dataContract": [
            {"entityName": "people", "fields": [
                {"name": "user_id", "type": "entity_link", "owner": true,
                 "references": {"entity": "core:users", "field": "id"}}
            ]},
            {"entityName": "items", "fields": [
                {"name": "person_id", "type": "entity_link",
                 "references": {"entity": "people", "field": "id"}},
                {"name": "status", "type": "text", "enum_values": ["open", "it's closed"],
                 "default_value": "open"},
                {"name": "tags", "type": "[text]", "enum_values": ["a", "b"]},
                {"name": "literal", "type": "text", "default_value": literal},
                {"name": "amount", "type": "decimal", "default_value": "19.20"},
                {"name": "metadata", "type": "json", "default_value": {"a": "it's literal"}},
                {"name": "end_date", "type": "date"}
            ], "indexes": [
                {"name": "status_order", "columns": [{"column": "status", "sort": "desc", "nulls": "last"}]}
            ]}
        ]
    });
    rt.install_manifest(&request).await;
    // Same shape emitted by the share compiler. No comment/name proof required.
    sqlx::query(
        "CREATE INDEX rootcx_share_admission_safe_items
         ON admission_safe.items (person_id, id) WHERE end_date IS NULL",
    )
    .execute(rt.pool())
    .await
    .unwrap();
    rt.install_manifest(&request).await;
    let value: String =
        sqlx::query_scalar("INSERT INTO admission_safe.items DEFAULT VALUES RETURNING literal")
            .fetch_one(rt.pool())
            .await
            .unwrap();
    assert_eq!(value, literal, "SQL-looking defaults remain data");
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_trigger t JOIN pg_class c ON c.oid=t.tgrelid
         JOIN pg_namespace n ON n.oid=c.relnamespace
         WHERE n.nspname=$1 AND NOT t.tgisinternal",
    )
    .bind(app)
    .fetch_one(rt.pool())
    .await
    .unwrap();
    assert!(
        count > 0,
        "fixture must exercise generated audit/hooks admission"
    );
    rt.shutdown().await;
}

#[tokio::test]
async fn pending_schemas_with_legacy_artifacts_are_refused_without_ddl_or_execution() {
    let rt = TestRuntime::boot().await;
    sqlx::query("CREATE TABLE rootcx_system.admission_probe (label text)")
        .execute(rt.pool())
        .await
        .unwrap();
    // This body is deliberately only stored, never called by test setup.
    sqlx::query(
        "CREATE FUNCTION rootcx_system.admission_probe_value() RETURNS text
         LANGUAGE plpgsql SECURITY DEFINER AS $$
         BEGIN INSERT INTO rootcx_system.admission_probe VALUES ('executed');
         RETURN '001.sql'; END $$",
    )
    .execute(rt.pool())
    .await
    .unwrap();

    let cases = [
        (
            "routine",
            "CREATE FUNCTION {s}.legacy() RETURNS text LANGUAGE sql SECURITY DEFINER AS 'SELECT rootcx_system.admission_probe_value()'",
        ),
        (
            "view",
            "CREATE VIEW {s}.legacy AS SELECT rootcx_system.admission_probe_value() AS value",
        ),
        (
            "materialized",
            "CREATE MATERIALIZED VIEW {s}.legacy AS SELECT rootcx_system.admission_probe_value() AS value WITH NO DATA",
        ),
        (
            "trigger",
            "CREATE TRIGGER legacy AFTER INSERT ON {s}.items FOR EACH ROW EXECUTE FUNCTION rootcx_system.audit_trigger_fn()",
        ),
        (
            "forged_trigger",
            "CREATE TRIGGER audit_{s}_items BEFORE INSERT ON {s}.items FOR EACH ROW EXECUTE FUNCTION rootcx_system.audit_trigger_fn()",
        ),
        (
            "policy",
            "CREATE POLICY allow_everything ON {s}.items USING (true)",
        ),
        (
            "rule",
            "CREATE RULE legacy AS ON DELETE TO {s}.items DO INSTEAD NOTHING",
        ),
        (
            "default",
            "ALTER TABLE {s}.items ALTER COLUMN status SET DEFAULT rootcx_system.admission_probe_value()",
        ),
        (
            "check",
            "ALTER TABLE {s}.items ADD CONSTRAINT legacy CHECK (length(status)>0) NOT VALID",
        ),
        (
            "index",
            "CREATE INDEX legacy ON {s}.items ((lower(status)))",
        ),
        (
            "predicate",
            "CREATE INDEX legacy ON {s}.items (status) WHERE status IS NOT NULL",
        ),
        (
            "type",
            "CREATE DOMAIN {s}.legacy AS text CHECK (VALUE IS NOT NULL)",
        ),
    ];
    for (name, ddl) in cases {
        let app = format!("legacy_{name}");
        sqlx::query(&format!("CREATE SCHEMA {app}"))
            .execute(rt.pool())
            .await
            .unwrap();
        sqlx::query(&format!("CREATE TABLE {app}.items (status text)"))
            .execute(rt.pool())
            .await
            .unwrap();
        sqlx::query(&ddl.replace("{s}", &app))
            .execute(rt.pool())
            .await
            .unwrap();
        if name == "check" {
            sqlx::query(&format!(
                "COMMENT ON CONSTRAINT legacy ON {app}.items IS 'rootcx:chk:forged'"
            ))
            .execute(rt.pool())
            .await
            .unwrap();
        }
        let mut request = manifest(&app);
        request["dataContract"]
            .as_array_mut()
            .unwrap()
            .push(json!({"entityName": "must_not_be_created", "fields": []}));
        let (status, body) = rt.post_json("/api/v1/apps", &request).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{name}: {body}");
        assert!(body.to_string().contains("SQL admission"), "{name}: {body}");
        let created: bool = sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
            .bind(format!("{app}.must_not_be_created"))
            .fetch_one(rt.pool())
            .await
            .unwrap();
        assert!(!created, "{name}: inspection must precede app DDL");
        let executions: i64 =
            sqlx::query_scalar("SELECT count(*) FROM rootcx_system.admission_probe")
                .fetch_one(rt.pool())
                .await
                .unwrap();
        assert_eq!(executions, 0, "{name}: admission evaluated legacy code");
    }
    rt.shutdown().await;
}

#[tokio::test]
async fn deploy_refuses_a_legacy_ledger_view_before_selecting_its_rows() {
    let rt = TestRuntime::boot().await;
    let app = "admission_ledger";
    rt.install_manifest(&manifest(app)).await;
    sqlx::query("CREATE TABLE rootcx_system.ledger_probe (label text)")
        .execute(rt.pool())
        .await
        .unwrap();
    sqlx::query(
        "CREATE FUNCTION rootcx_system.ledger_probe_value() RETURNS text
         LANGUAGE plpgsql SECURITY DEFINER AS $$
         BEGIN INSERT INTO rootcx_system.ledger_probe VALUES ('executed');
         RETURN '001.sql'; END $$",
    )
    .execute(rt.pool())
    .await
    .unwrap();
    sqlx::query(
        "CREATE VIEW admission_ledger.schema_migrations
         AS SELECT rootcx_system.ledger_probe_value() AS filename",
    )
    .execute(rt.pool())
    .await
    .unwrap();

    let mut archive = tar::Builder::new(Vec::new());
    // An already-applied filename would cause the old ledger gate to accept it.
    // The file itself is harmless and must not run.
    let body = b"SELECT 1;";
    let mut header = tar::Header::new_gnu();
    header.set_size(body.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    archive
        .append_data(&mut header, "migrations/001.sql", &body[..])
        .unwrap();
    let tar = archive.into_inner().unwrap();
    let mut compressed = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    std::io::Write::write_all(&mut compressed, &tar).unwrap();
    let (status, body) = rt.deploy(app, &compressed.finish().unwrap()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.to_string().contains("SQL admission"), "{body}");
    let executions: i64 = sqlx::query_scalar("SELECT count(*) FROM rootcx_system.ledger_probe")
        .fetch_one(rt.pool())
        .await
        .unwrap();
    assert_eq!(executions, 0, "ledger was evaluated before admission");
    rt.shutdown().await;
}
