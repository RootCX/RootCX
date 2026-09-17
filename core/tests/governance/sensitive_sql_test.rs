//! Sensitive columns are write-only through generated CRUD and arbitrary Bun SQL.
use crate::harness::{self, TestRuntime};
use reqwest::{Method, StatusCode};
use rootcx_core::extensions::{RuntimeExtension, rbac::RbacExtension};
use serde_json::{Value, json};
use uuid::Uuid;

const COLLECTION: &str = "/api/v1/apps/vault/collections/records";
const EXACT: &str = "9007199254740993.12345678";
const BACKEND: &[u8] = br#"
serve({ rpc: {
  probe: async (params, _caller, ctx) => {
    try {
      const result = params.op === "sql"
        ? await ctx.sql(params.sql, params.params ?? [])
        : await ctx.collection("records")[params.op](...(params.args ?? []));
      return { result };
    } catch (error) { return { error: error.message }; }
  },
} });
"#;

fn manifest(sensitive: bool) -> Value {
    json!({
        "appId": "vault", "name": "vault", "version": "1.0.0",
        "dataContract": [{"entityName": "records", "fields": [
            {"name": "label", "type": "text"},
            {"name": "exact_amount", "type": "decimal", "precision": 30, "scale": 8},
            {"name": "nullable_amount", "type": "decimal"},
            {"name": "enabled", "type": "boolean"},
            {"name": "metadata", "type": "json"},
            {"name": "tags", "type": "[text]"},
            {"name": "secret", "type": "text", "sensitive": sensitive},
            {"name": "secret_amount", "type": "decimal", "sensitive": sensitive}
        ]}]
    })
}

async fn fixture(manifest: &Value) -> (TestRuntime, String) {
    let rt = TestRuntime::boot().await;
    rt.install_manifest(manifest).await;
    let token = rt.register_and_login("sensitive-sql@test.local").await;
    let user: Uuid = sqlx::query_scalar(
        "SELECT id FROM rootcx_system.users WHERE email = 'sensitive-sql@test.local'",
    )
    .fetch_one(rt.pool())
    .await
    .unwrap();
    sqlx::query("DELETE FROM rootcx_system.rbac_assignments WHERE user_id = $1")
        .bind(user)
        .execute(rt.pool())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO rootcx_system.rbac_roles (name, permissions)
         VALUES ('vault_user', ARRAY[
             'app:vault:invoke', 'app:vault:records.read', 'app:vault:records.create',
             'app:vault:records.update', 'app:vault:records.delete'
         ])",
    )
    .execute(rt.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO rootcx_system.rbac_assignments (user_id, role) VALUES ($1, 'vault_user')",
    )
    .bind(user)
    .execute(rt.pool())
    .await
    .unwrap();
    let (status, body) = deploy(&rt).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    (rt, token)
}

async fn deploy(rt: &TestRuntime) -> (StatusCode, Value) {
    rt.deploy("vault", &harness::make_tar_gz(&[("index.ts", BACKEND)]))
        .await
}

async fn call(rt: &TestRuntime, token: &str, params: Value) -> (StatusCode, Value) {
    rt.request_as(
        Method::POST,
        "/api/v1/apps/vault/rpc",
        token,
        Some(&json!({"method": "probe", "params": params})),
    )
    .await
}

async fn seed(rt: &TestRuntime) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO vault.records (label, exact_amount, secret, secret_amount)
         VALUES ('readable', $1::numeric, 'classified', 123.456) RETURNING id",
    )
    .bind(EXACT)
    .fetch_one(rt.pool())
    .await
    .unwrap()
}

fn record(label: &str) -> Value {
    json!({
        "label": label, "exact_amount": EXACT, "nullable_amount": null,
        "enabled": true, "metadata": {"visible": [1, 2]}, "tags": ["a", "b"],
        "secret": "classified", "secret_amount": "123.456"
    })
}

#[tokio::test]
async fn worker_sql_denies_sensitive_references_even_for_admins() {
    let (rt, token) = fixture(&manifest(true)).await;
    seed(&rt).await;
    for (caller, token) in [("user", token.as_str()), ("admin", rt.token.as_str())] {
        for sql in [
            "SELECT secret FROM vault.records",
            "SELECT secret AS label FROM vault.records",
            "SELECT upper(secret) AS label FROM vault.records",
            "SELECT secret_amount + 1 AS total FROM vault.records",
            "SELECT label FROM vault.records WHERE secret = 'classified'",
            "SELECT label FROM vault.records WHERE secret IS NULL",
            "SELECT count(secret) FROM vault.records",
            "SELECT sum(secret_amount) FROM vault.records",
            "SELECT label FROM vault.records GROUP BY label HAVING max(secret) = 'classified'",
            "SELECT label FROM vault.records ORDER BY secret",
            "SELECT DISTINCT ON (secret) label FROM vault.records",
            "SELECT EXISTS (SELECT 1 FROM vault.records WHERE secret = 'classified')",
            "SELECT * FROM vault.records",
            "SELECT r.* FROM vault.records r",
            "SELECT r FROM vault.records r",
            "SELECT to_jsonb(r) - 'secret' - 'secret_amount' FROM vault.records r",
            "SELECT row_to_json(r) FROM vault.records r",
            "INSERT INTO vault.records (label, secret) VALUES ('denied', 'new') RETURNING secret",
            "INSERT INTO vault.records (label, secret) VALUES ('denied', 'new') RETURNING *",
            "UPDATE vault.records SET secret = 'changed' RETURNING secret AS label",
            "UPDATE vault.records SET secret = 'changed' RETURNING *",
            "UPDATE vault.records SET secret = secret || 'changed' RETURNING label",
            "UPDATE vault.records SET label = 'denied' WHERE secret = 'classified' RETURNING label",
            "DELETE FROM vault.records RETURNING secret",
            "DELETE FROM vault.records RETURNING *",
        ] {
            let (status, body) = call(&rt, token, json!({"op": "sql", "sql": sql})).await;
            assert_eq!(status, StatusCode::OK, "{caller}/{sql}: {body}");
            assert!(
                body["error"]
                    .as_str()
                    .is_some_and(|e| e.contains("permission denied")),
                "{caller}/{sql}: expected PostgreSQL column denial, got {body}"
            );
            assert!(body.get("result").is_none(), "{caller}/{sql}: {body}");
        }
        let (status, body) = call(
            &rt,
            token,
            json!({"op": "sql", "sql":
                "SELECT label, exact_amount FROM vault.records WHERE label = $1",
                "params": ["readable"]}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{caller}: {body}");
        assert_eq!(
            body["result"]["rows"],
            json!([["readable", EXACT]]),
            "{caller}: {body}"
        );
    }
    let stored: Vec<(String, String)> = sqlx::query_as("SELECT label, secret FROM vault.records")
        .fetch_all(rt.pool())
        .await
        .unwrap();
    assert_eq!(
        stored,
        vec![("readable".into(), "classified".into())],
        "denied RETURNING and predicates must not commit mutations"
    );
    rt.shutdown().await;
}

#[tokio::test]
async fn worker_sql_allows_safe_queries_and_sensitive_writes_with_safe_returning() {
    let (rt, token) = fixture(&manifest(true)).await;
    for (sql, params, expected) in [
        (
            "INSERT INTO vault.records (label, secret, secret_amount) VALUES ($1, $2, $3) RETURNING label",
            json!(["inserted", "classified", "123.456"]),
            json!([["inserted"]]),
        ),
        (
            "UPDATE vault.records SET secret = $1, label = $2 WHERE label = $3 RETURNING label",
            json!(["changed", "updated", "inserted"]),
            json!([["updated"]]),
        ),
        (
            "SELECT upper(label) AS display FROM vault.records WHERE label = $1",
            json!(["updated"]),
            json!([["UPDATED"]]),
        ),
        (
            "SELECT count(*)::int FROM vault.records",
            json!([]),
            json!([[1]]),
        ),
    ] {
        let (status, body) = call(
            &rt,
            &token,
            json!({"op": "sql", "sql": sql, "params": params}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{sql}: {body}");
        assert_eq!(body["result"]["rows"], expected, "{sql}: {body}");
    }
    let secret: String = sqlx::query_scalar("SELECT secret FROM vault.records")
        .fetch_one(rt.pool())
        .await
        .unwrap();
    assert_eq!(
        secret, "changed",
        "safe RETURNING must commit sensitive writes"
    );
    let (_, body) = call(
        &rt,
        &token,
        json!({"op": "sql", "sql": "DELETE FROM vault.records RETURNING label"}),
    )
    .await;
    assert_eq!(body["result"]["rows"], json!([["updated"]]), "{body}");
    rt.shutdown().await;
}

#[tokio::test]
async fn worker_crud_returns_safe_typed_rows_and_rejects_sensitive_filters() {
    let (rt, token) = fixture(&manifest(true)).await;
    for create in ["insert", "create"] {
        let (status, body) =
            call(&rt, &token, json!({"op": create, "args": [record(create)]})).await;
        assert_eq!(status, StatusCode::OK, "{create}: {body}");
        let created = &body["result"];
        let id = created["id"].as_str().expect("generated insert returns id");
        let mut expected = record(create);
        expected.as_object_mut().unwrap().remove("secret");
        expected.as_object_mut().unwrap().remove("secret_amount");
        for key in ["id", "created_at", "updated_at"] {
            assert!(created[key].is_string(), "{create}/{key}: {body}");
            expected[key] = created[key].clone();
        }
        assert_eq!(created, &expected, "{create}: {body}");
        let stored: (String, String) =
            sqlx::query_as("SELECT secret, secret_amount::text FROM vault.records WHERE id = $1")
                .bind(Uuid::parse_str(id).unwrap())
                .fetch_one(rt.pool())
                .await
                .unwrap();
        assert_eq!(stored, ("classified".into(), "123.456".into()), "{create}");

        for (op, args) in [
            ("find", json!([{"id": id}])),
            ("findOne", json!([{"id": id}])),
            ("findPage", json!([{"where": {"id": id}, "limit": 1}])),
        ] {
            let (status, body) = call(&rt, &token, json!({"op": op, "args": args})).await;
            assert_eq!(status, StatusCode::OK, "{op}: {body}");
            let row = match op {
                "find" => &body["result"][0],
                "findPage" => {
                    assert_eq!(body["result"]["total"], 1, "{body}");
                    &body["result"]["data"][0]
                }
                _ => &body["result"],
            };
            assert_eq!(row, &expected, "{op}: {body}");
        }
        for args in [
            json!([id, {"secret": "changed", "secret_amount": "456.789"}]),
            json!([{"id": id, "secret": "changed", "secret_amount": "456.789"}]),
        ] {
            let (status, updated) = call(&rt, &token, json!({"op": "update", "args": args})).await;
            assert_eq!(status, StatusCode::OK, "{updated}");
            assert_eq!(updated["result"]["label"], create, "{updated}");
            assert_eq!(updated["result"]["exact_amount"], EXACT, "{updated}");
            for secret in ["secret", "secret_amount"] {
                assert!(updated["result"].get(secret).is_none(), "{updated}");
            }
            let stored: (String, String) = sqlx::query_as(
                "SELECT secret, secret_amount::text FROM vault.records WHERE id = $1",
            )
            .bind(Uuid::parse_str(id).unwrap())
            .fetch_one(rt.pool())
            .await
            .unwrap();
            assert_eq!(stored, ("changed".into(), "456.789".into()));
        }
        let (_, deleted) = call(&rt, &token, json!({"op": "delete", "args": [id]})).await;
        assert_eq!(
            deleted["result"],
            json!({"id": id, "deleted": true}),
            "{deleted}"
        );
    }
    for (op, args) in [
        ("find", json!([{"secret": "classified"}])),
        ("findOne", json!([{"secret_amount": "123.456"}])),
        ("findPage", json!([{"where": {"secret": "classified"}}])),
        ("findPage", json!([{"orderBy": "secret"}])),
    ] {
        let (_, body) = call(&rt, &token, json!({"op": op, "args": args})).await;
        assert!(
            body["error"]
                .as_str()
                .is_some_and(|e| e.contains("sensitive")),
            "{op}: {body}"
        );
    }
    rt.shutdown().await;
}

#[tokio::test]
async fn http_collections_write_secrets_and_return_only_safe_values() {
    let (rt, token) = fixture(&manifest(true)).await;
    let (status, created) = rt
        .request_as(Method::POST, COLLECTION, &token, Some(&record("http")))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().expect("created id");
    let path = format!("{COLLECTION}/{id}");
    let mut expected = record("http");
    expected.as_object_mut().unwrap().remove("secret");
    expected.as_object_mut().unwrap().remove("secret_amount");
    for key in ["id", "created_at", "updated_at"] {
        assert!(created[key].is_string(), "{key}: {created}");
        expected[key] = created[key].clone();
    }
    assert_eq!(created, expected);
    let stored: (String, String) =
        sqlx::query_as("SELECT secret, secret_amount::text FROM vault.records WHERE id = $1")
            .bind(Uuid::parse_str(id).unwrap())
            .fetch_one(rt.pool())
            .await
            .unwrap();
    assert_eq!(stored, ("classified".into(), "123.456".into()));
    for (method, path, payload) in [
        (Method::GET, COLLECTION.to_string(), None),
        (Method::GET, path.clone(), None),
        (
            Method::POST,
            format!("{COLLECTION}/query"),
            Some(json!({"where": {"id": id}})),
        ),
    ] {
        let (status, body) = rt.request_as(method, &path, &token, payload.as_ref()).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
        let row = if body.is_array() {
            &body[0]
        } else if body.get("data").is_some() {
            assert_eq!(body["total"], 1, "{body}");
            &body["data"][0]
        } else {
            &body
        };
        assert_eq!(row, &expected, "{path}: {body}");
    }
    let (status, updated) = rt
        .request_as(
            Method::PATCH,
            &path,
            &token,
            Some(&json!({"secret": "changed", "secret_amount": "456.789"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    expected["updated_at"] = updated["updated_at"].clone();
    assert_eq!(updated, expected);
    let stored: (String, String) =
        sqlx::query_as("SELECT secret, secret_amount::text FROM vault.records WHERE id = $1")
            .bind(Uuid::parse_str(id).unwrap())
            .fetch_one(rt.pool())
            .await
            .unwrap();
    assert_eq!(stored, ("changed".into(), "456.789".into()));

    let (status, bulk) = rt
        .request_as(
            Method::POST,
            &format!("{COLLECTION}/bulk"),
            &token,
            Some(&json!([record("bulk-one"), record("bulk-two")])),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{bulk}");
    let rows = bulk.as_array().expect("bulk returns rows");
    assert_eq!(rows.len(), 2, "{bulk}");
    for row in rows {
        assert_eq!(row["exact_amount"], EXACT, "{row}");
        assert_eq!(row.get("nullable_amount"), Some(&Value::Null), "{row}");
        assert!(row.get("secret").is_none(), "{row}");
        assert!(row.get("secret_amount").is_none(), "{row}");
    }
    let stored: Vec<(String, String)> = sqlx::query_as(
        "SELECT secret, secret_amount::text FROM vault.records WHERE label LIKE 'bulk-%'",
    )
    .fetch_all(rt.pool())
    .await
    .unwrap();
    assert_eq!(stored, vec![("classified".into(), "123.456".into()); 2]);
    for (method, path, payload) in [
        (Method::GET, format!("{COLLECTION}?secret=classified"), None),
        (
            Method::POST,
            format!("{COLLECTION}/query"),
            Some(json!({
                "where": {"$or": [{"label": "http"}, {"secret": "classified"}]}
            })),
        ),
    ] {
        let (status, body) = rt.request_as(method, &path, &token, payload.as_ref()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path}: {body}");
    }
    let (status, body) = rt.request_as(Method::DELETE, &path, &token, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    rt.shutdown().await;
}

#[tokio::test]
async fn wide_projections_exclude_migration_columns_with_or_without_sensitive_fields() {
    let mut declaration = manifest(true);
    let fields = declaration["dataContract"][0]["fields"]
        .as_array_mut()
        .unwrap();
    // More than one function's argument budget, including a non-full last chunk.
    for index in 0..85 {
        fields.push(json!({"name": format!("extra_{index:03}"), "type": "number"}));
    }
    let (rt, token) = fixture(&declaration).await;
    // Simulate a column introduced by a trusted administrator before upgrade.
    // Backend archives can no longer execute owner SQL.
    sqlx::query("ALTER TABLE vault.records ADD COLUMN migration_private TEXT DEFAULT 'migration-only'")
        .execute(rt.pool()).await.unwrap();
    let id = seed(&rt).await;
    let private: String =
        sqlx::query_scalar("SELECT migration_private FROM vault.records WHERE id = $1")
            .bind(id)
            .fetch_one(rt.pool())
            .await
            .unwrap();
    assert_eq!(
        private, "migration-only",
        "migration must really add an undeclared column"
    );

    for sensitive in [true, false] {
        for field in declaration["dataContract"][0]["fields"]
            .as_array_mut()
            .unwrap()
        {
            if matches!(field["name"].as_str(), Some("secret" | "secret_amount")) {
                field["sensitive"] = json!(sensitive);
            }
        }
        rt.install_manifest(&declaration).await;
        let (status, http) = rt
            .request_as(Method::GET, &format!("{COLLECTION}/{id}"), &token, None)
            .await;
        assert_eq!(status, StatusCode::OK, "sensitive={sensitive}: {http}");
        let (status, worker) =
            call(&rt, &token, json!({"op": "findOne", "args": [{"id": id}]})).await;
        assert_eq!(status, StatusCode::OK, "sensitive={sensitive}: {worker}");
        for row in [&http, &worker["result"]] {
            assert_eq!(row["label"], "readable", "{row}");
            assert_eq!(row["exact_amount"], EXACT, "{row}");
            assert_eq!(row.get("nullable_amount"), Some(&Value::Null), "{row}");
            assert!(row.get("migration_private").is_none(), "{row}");
            assert_eq!(row.get("secret").is_none(), sensitive, "{row}");
            assert_eq!(row.get("secret_amount").is_none(), sensitive, "{row}");
            for index in 0..85 {
                let key = format!("extra_{index:03}");
                assert_eq!(row.get(&key), Some(&Value::Null), "{key}: {row}");
            }
            assert_eq!(
                row.as_object().unwrap().len(),
                if sensitive { 94 } else { 96 }
            );
        }
    }
    rt.shutdown().await;
}

#[tokio::test]
async fn sensitivity_toggles_reconcile_column_privileges_on_redeploy_and_bootstrap() {
    let (rt, token) = fixture(&manifest(false)).await;
    let id = seed(&rt).await;
    for sensitive in [false, true, false, true] {
        rt.install_manifest(&manifest(sensitive)).await;
        let (status, body) = deploy(&rt).await;
        assert_eq!(status, StatusCode::OK, "sensitive={sensitive}: {body}");
        // Governing another entity in the schema must not restore a schema-wide
        // SELECT grant on the table that was just restricted.
        let mut expanded = manifest(sensitive);
        expanded["dataContract"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "entityName": "other", "fields": [{"name": "label", "type": "text"}]
            }));
        rt.install_manifest(&expanded).await;
        for phase in ["redeploy", "bootstrap"] {
            if phase == "bootstrap" {
                // Simulate a pre-upgrade table grant and a stale explicit column
                // grant; revoking only the table grant cannot repair the latter.
                sqlx::query("GRANT SELECT ON vault.records TO rootcx_app_executor")
                    .execute(rt.pool())
                    .await
                    .unwrap();
                sqlx::query(
                    "GRANT SELECT (secret, secret_amount) ON vault.records TO rootcx_app_executor",
                )
                .execute(rt.pool())
                .await
                .unwrap();
                RbacExtension
                    .bootstrap(rt.pool())
                    .await
                    .expect("bootstrap reconciles privileges");
            }
            let (status, body) = call(
                &rt,
                &token,
                json!({"op": "sql", "sql": "SELECT secret, secret_amount FROM vault.records"}),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{phase}/{sensitive}: {body}");
            if sensitive {
                assert!(
                    body["error"]
                        .as_str()
                        .is_some_and(|e| e.contains("permission denied")),
                    "{phase}/{sensitive}: {body}"
                );
            } else {
                assert_eq!(body["result"]["rows"][0][0], "classified", "{phase}: {body}");
                let amount = body["result"]["rows"][0][1].as_str().unwrap()
                    .parse::<sqlx::types::BigDecimal>().unwrap();
                assert_eq!(amount, "123.456".parse::<sqlx::types::BigDecimal>().unwrap(),
                    "{phase}/{sensitive}: {body}");
            }
            let (status, row) = rt
                .request_as(Method::GET, &format!("{COLLECTION}/{id}"), &token, None)
                .await;
            assert_eq!(status, StatusCode::OK, "{phase}/{sensitive}: {row}");
            assert_eq!(row["label"], "readable", "{phase}/{sensitive}: {row}");
            assert_eq!(row["exact_amount"], EXACT, "{phase}/{sensitive}: {row}");
            assert_eq!(row.get("secret").is_none(), sensitive, "{phase}: {row}");
            assert_eq!(
                row.get("secret_amount").is_none(),
                sensitive,
                "{phase}: {row}"
            );
        }
    }
    rt.shutdown().await;
}

#[tokio::test]
async fn shared_sensitive_rows_revoke_without_hiding_the_subjects_own_safe_rows() {
    let rt = TestRuntime::boot().await;
    let mut declaration = manifest(true);
    declaration["dataContract"][0]["fields"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "name": "enrollment_id", "type": "entity_link", "owner": true,
            "references": {"entity": "enrollment", "field": "id"}
        }));
    let records = declaration["dataContract"]
        .as_array_mut()
        .unwrap()
        .remove(0);
    declaration["dataContract"].as_array_mut().unwrap().extend([
        json!({"entityName": "person", "fields": [
            {"name": "core_user_id", "type": "entity_link", "owner": true,
             "references": {"entity": "core:users", "field": "id"}}
        ]}),
        json!({"entityName": "enrollment", "fields": [
            {"name": "person_id", "type": "entity_link", "owner": true,
             "references": {"entity": "person", "field": "id"}}
        ]}),
        records,
        json!({"entityName": "assignment", "share": {
            "grantee": "helper_enrollment_id", "subject": "helped_enrollment_id",
            "activeWhen": {"isNull": "end_date"}
        }, "fields": [
            {"name": "helper_enrollment_id", "type": "entity_link",
             "references": {"entity": "enrollment", "field": "id"}},
            {"name": "helped_enrollment_id", "type": "entity_link",
             "references": {"entity": "enrollment", "field": "id"}},
            {"name": "end_date", "type": "date"}
        ]}),
    ]);
    rt.install_manifest(&declaration).await;

    let mut people = Vec::new();
    // Own and unrelated control rows catch accidental unscoped or .own access
    // for the helper, who has only the target entity's .read.shared permission.
    for (label, scope) in [
        ("helper", "shared"),
        ("subject", "own"),
        ("outsider", "own"),
    ] {
        let email = format!("{label}@sensitive-sharing.test");
        let token = rt.register_and_login(&email).await;
        let user: Uuid = sqlx::query_scalar("SELECT id FROM rootcx_system.users WHERE email = $1")
            .bind(&email)
            .fetch_one(rt.pool())
            .await
            .unwrap();
        sqlx::query("DELETE FROM rootcx_system.rbac_assignments WHERE user_id = $1")
            .bind(user)
            .execute(rt.pool())
            .await
            .unwrap();
        let role = format!("sensitive_{label}");
        sqlx::query("INSERT INTO rootcx_system.rbac_roles (name, permissions) VALUES ($1, $2)")
            .bind(&role)
            .bind(vec![
                "app:vault:invoke".to_string(),
                format!("app:vault:records.read.{scope}"),
            ])
            .execute(rt.pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO rootcx_system.rbac_assignments (user_id, role) VALUES ($1, $2)")
            .bind(user)
            .bind(role)
            .execute(rt.pool())
            .await
            .unwrap();
        let person = rt
            .create("vault", "person", &json!({"core_user_id": user}))
            .await;
        let enrollment = rt
            .create("vault", "enrollment", &json!({"person_id": person["id"]}))
            .await;
        let mut payload = record(label);
        payload["enrollment_id"] = enrollment["id"].clone();
        let row = rt.create("vault", "records", &payload).await;
        people.push((token, enrollment["id"].clone(), row["id"].clone()));
    }
    let (helper_token, helper_enrollment, _) = &people[0];
    let (subject_token, subject_enrollment, subject_record) = &people[1];
    let assignment = rt
        .create(
            "vault",
            "assignment",
            &json!({
                "helper_enrollment_id": helper_enrollment,
                "helped_enrollment_id": subject_enrollment,
                "end_date": null
            }),
        )
        .await;
    let (status, body) = deploy(&rt).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    for active in [true, false] {
        if !active {
            // Any non-null end_date ends this relation, even a future date.
            let (status, body) = rt
                .patch_json(
                    &format!(
                        "/api/v1/apps/vault/collections/assignment/{}",
                        assignment["id"].as_str().unwrap()
                    ),
                    &json!({"end_date": "9999-12-31"}),
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body["end_date"], "9999-12-31", "{body}");
        }
        for (caller, token, visible) in [
            ("helper.shared", helper_token, active),
            ("subject.own", subject_token, true),
        ] {
            let (status, body) = rt.request_as(Method::GET, COLLECTION, token, None).await;
            assert_eq!(status, StatusCode::OK, "{caller}/active={active}: {body}");
            let rows = body.as_array().expect("collections returns rows");
            assert_eq!(
                rows.len(),
                usize::from(visible),
                "{caller}/active={active}: {body}"
            );
            for row in rows {
                assert_eq!(&row["id"], subject_record, "{caller}: {row}");
                assert_eq!(row["label"], "subject", "{caller}: {row}");
                assert_eq!(row["exact_amount"], EXACT, "{caller}: {row}");
                assert!(row.get("secret").is_none(), "{caller}: {row}");
                assert!(row.get("secret_amount").is_none(), "{caller}: {row}");
            }
            let (status, body) = call(
                &rt,
                token,
                json!({"op": "sql", "sql":
                    "SELECT id::text, label, exact_amount FROM vault.records ORDER BY id"}),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{caller}/active={active}: {body}");
            let expected = if visible {
                json!([[subject_record, "subject", EXACT]])
            } else {
                json!([])
            };
            assert_eq!(
                body["result"]["rows"], expected,
                "{caller}/active={active}: {body}"
            );
            for sql in [
                "SELECT secret FROM vault.records",
                "SELECT secret AS label FROM vault.records",
                "SELECT label FROM vault.records WHERE secret = 'classified'",
            ] {
                let (status, body) = call(&rt, token, json!({"op": "sql", "sql": sql})).await;
                assert_eq!(
                    status,
                    StatusCode::OK,
                    "{caller}/active={active}/{sql}: {body}"
                );
                assert!(
                    body["error"]
                        .as_str()
                        .is_some_and(|e| e.contains("permission denied")),
                    "{caller}/active={active}/{sql}: {body}"
                );
                assert!(body.get("result").is_none(), "{caller}/{sql}: {body}");
            }
        }
    }
    rt.shutdown().await;
}
