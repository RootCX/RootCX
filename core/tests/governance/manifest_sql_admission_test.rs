//! New manifest SQL stays bounded; historical schema objects survive boot.

use crate::harness::TestRuntime;
use reqwest::StatusCode;
use rootcx_core::extensions::{RuntimeExtension, rbac::RbacExtension};
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
        (
            "breakout",
            json!({"checks": [{"name": "c", "expr": "true), NO FORCE ROW LEVEL SECURITY, DISABLE ROW LEVEL SECURITY, ADD CONSTRAINT zz CHECK (true"}]}),
        ),
        (
            "owner_function",
            json!({"checks": [{"name": "c", "expr": "pg_read_file('/etc/passwd') IS NOT NULL"}]}),
        ),
        ("cast", json!({"checks": [{"name": "c", "expr": "status::int > 0"}]})),
        ("unnamed_check", json!({"checks": [{"expr": "status IS NOT NULL"}]})),
        (
            "expression",
            json!({"indexes": [{"columns": [{"expr": "query_to_xml(status, true, true, '')"}]}]}),
        ),
        (
            "predicate",
            json!({"indexes": [{"columns": ["status"], "where": "true) WITH (fillfactor=10"}]}),
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
    // Same shape emitted by the share compiler.
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
        "fixture must exercise generated audit/hooks on reinstall"
    );
    rt.shutdown().await;
}

#[tokio::test]
async fn legacy_schema_declarations_survive_bootstrap_without_losing_constraints() {
    let rt = TestRuntime::boot().await;
    let app = "legacy_calendar";
    let request = json!({
        "appId": app, "name": app,
        "dataContract": [{"entityName": "event_attachments", "fields": [
            {"name": "event_id", "type": "text"},
            {"name": "file_id", "type": "text"},
            {"name": "status", "type": "text"},
            {"name": "active", "type": "boolean"},
            {"name": "position", "type": "number"}
        ]}]
    });
    rt.install_manifest(&request).await;
    sqlx::query(
        "CREATE UNIQUE INDEX idx_gcal_attach_unique
         ON legacy_calendar.event_attachments (event_id, file_id)
         WHERE file_id IS NOT NULL AND file_id <> ''",
    ).execute(rt.pool()).await.unwrap();
    for (name, predicate) in [
        ("not_null", "file_id IS NOT NULL"),
        ("boolean", "active = true AND file_id IS NULL"),
        ("boolean_test", "active IS TRUE OR NOT active"),
        ("numeric", "position > 0::double precision"),
        ("statuses", "status = ANY(ARRAY['queued','publishing']::text[])"),
        ("exclusions", "status <> ALL(ARRAY['failed','deleting']::text[])"),
    ] {
        sqlx::query(&format!(
            "CREATE INDEX {name} ON legacy_calendar.event_attachments (event_id) WHERE {predicate}"
        )).execute(rt.pool()).await.unwrap();
    }
    sqlx::query(
        "INSERT INTO legacy_calendar.event_attachments (event_id, file_id)
         VALUES ('event', NULL), ('event', NULL), ('event', ''), ('event', ''), ('event', 'file')",
    ).execute(rt.pool()).await.unwrap();
    let before: Vec<(String, String)> = sqlx::query_as(
        "SELECT indexname::text, indexdef::text FROM pg_indexes
         WHERE schemaname='legacy_calendar' ORDER BY indexname",
    ).fetch_all(rt.pool()).await.unwrap();

    sqlx::query("ALTER TABLE legacy_calendar.event_attachments ADD CONSTRAINT valid_position CHECK (position >= 0)")
        .execute(rt.pool()).await.unwrap();
    let mut historical = request.clone();
    // Old manifests also used type aliases; replay must not recreate columns.
    historical["dataContract"][0]["fields"][2]["type"] = json!("string");
    historical["dataContract"][0]["checks"] = json!([{"name": "valid_position", "expr": "position >= 0"}]);
    historical["dataContract"][0]["indexes"] = json!([{
        "name": "idx_gcal_attach_unique", "columns": ["event_id", "file_id"], "unique": true,
        "where": "file_id IS NOT NULL AND file_id <> ''"
    }]);
    // A pre-0.27 tenant has stored SQL declarations but no access projection.
    sqlx::query("UPDATE rootcx_system.apps SET manifest=$2 WHERE id=$1")
        .bind(app).bind(&historical).execute(rt.pool()).await.unwrap();
    sqlx::query("DELETE FROM rootcx_system.row_access_contracts WHERE app_id=$1")
        .bind(app).execute(rt.pool()).await.unwrap();
    RbacExtension.bootstrap(rt.pool()).await.expect("legacy partial indexes must not prevent boot");
    RbacExtension.bootstrap(rt.pool()).await.expect("the saved projection must also survive restart");
    let (status, body) = rt.post_json("/api/v1/apps", &historical).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "a stored type alias is not a new declaration: {body}");

    let after: Vec<(String, String)> = sqlx::query_as(
        "SELECT indexname::text, indexdef::text FROM pg_indexes
         WHERE schemaname='legacy_calendar' ORDER BY indexname",
    ).fetch_all(rt.pool()).await.unwrap();
    assert_eq!(after, before, "bootstrap must not remove or rewrite business indexes");
    let error = sqlx::query(
        "INSERT INTO legacy_calendar.event_attachments (event_id, file_id) VALUES ('event', 'file')",
    ).execute(rt.pool()).await.expect_err("duplicate attachments must remain forbidden");
    assert_eq!(error.as_database_error().and_then(|e| e.code()).as_deref(), Some("23505"));
    let error = sqlx::query(
        "INSERT INTO legacy_calendar.event_attachments (event_id, position) VALUES ('negative', -1)",
    ).execute(rt.pool()).await.expect_err("historical CHECK must remain enforced");
    assert_eq!(error.as_database_error().and_then(|e| e.code()).as_deref(), Some("23514"));
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM legacy_calendar.event_attachments")
        .fetch_one(rt.pool()).await.unwrap();
    assert_eq!(rows, 5, "legacy data and the NULL/empty-file exceptions must survive");
    rt.shutdown().await;
}
