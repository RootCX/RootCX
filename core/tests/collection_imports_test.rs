mod harness;

use futures_util::stream;
use harness::TestRuntime;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

async fn request_import(
    rt: &TestRuntime,
    app: &str,
    entity: &str,
    body: Value,
) -> (StatusCode, Value) {
    rt.post_json(
        &format!("/api/v1/apps/{app}/collections/{entity}/imports"),
        &body,
    )
    .await
}

async fn upload_csv(rt: &TestRuntime, import: &Value, csv: &str) -> (StatusCode, Value) {
    let response = rt
        .client
        .post(import["upload_url"].as_str().expect("upload URL"))
        .header("content-type", "text/csv")
        .body(csv.to_string())
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body = response.json().await.unwrap_or(Value::Null);
    (status, body)
}

#[tokio::test]
async fn append_publishes_rows_with_one_summary_audit_event() {
    let rt = TestRuntime::boot().await;
    rt.install("warehouse", "items").await;

    let (create_status, import) = request_import(
        &rt,
        "warehouse",
        "items",
        json!({
            "mode": "append",
            "columns": ["first_name", "last_name", "email"]
        }),
    )
    .await;
    assert_eq!(create_status, StatusCode::ACCEPTED, "create: {import}");

    let (upload_status, result) = upload_csv(
        &rt,
        &import,
        "Jerome,Kova,jerome@kova.test\nSandro,Munda,sandro@munda.me\n",
    )
    .await;
    assert_eq!(upload_status, StatusCode::OK, "upload: {result}");
    assert_eq!(result["rows_loaded"], 2);

    let row_count: i64 = sqlx::query_scalar("SELECT count(*) FROM warehouse.items")
        .fetch_one(rt.pool())
        .await
        .unwrap();
    assert_eq!(row_count, 2);

    let audit_rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT operation, record_id FROM rootcx_system.audit_log
         WHERE table_schema = 'warehouse' AND table_name = 'items'
         ORDER BY id",
    )
    .fetch_all(rt.pool())
    .await
    .unwrap();
    assert_eq!(
        audit_rows,
        vec![(
            "BULK_IMPORT".to_string(),
            import["id"].as_str().unwrap().to_string()
        )]
    );
    rt.shutdown().await;
}

#[tokio::test]
async fn idempotency_key_binds_one_request_shape() {
    let rt = TestRuntime::boot().await;
    rt.install("idempotent", "items").await;
    let request = json!({
        "mode": "append",
        "columns": ["first_name", "last_name"],
        "idempotencyKey": "supplier-file-sha256:mapping-v1"
    });

    let ((left_status, left), (right_status, right)) = tokio::join!(
        request_import(&rt, "idempotent", "items", request.clone()),
        request_import(&rt, "idempotent", "items", request),
    );
    assert_eq!(left_status, StatusCode::ACCEPTED, "left: {left}");
    assert_eq!(right_status, StatusCode::ACCEPTED, "right: {right}");
    assert_eq!(left["id"], right["id"]);

    let (mismatch_status, mismatch) = request_import(
        &rt,
        "idempotent",
        "items",
        json!({
            "mode": "append",
            "columns": ["first_name"],
            "idempotencyKey": "supplier-file-sha256:mapping-v1"
        }),
    )
    .await;
    assert_eq!(
        mismatch_status,
        StatusCode::CONFLICT,
        "mismatch: {mismatch}"
    );
    rt.shutdown().await;
}

#[tokio::test]
async fn upload_token_cannot_be_replayed_after_publication() {
    let rt = TestRuntime::boot().await;
    rt.install("single_use", "items").await;
    let (create_status, import) = request_import(
        &rt,
        "single_use",
        "items",
        json!({ "mode": "append", "columns": ["first_name", "last_name"] }),
    )
    .await;
    assert_eq!(create_status, StatusCode::ACCEPTED, "create: {import}");
    let upload_url = import["upload_url"].as_str().unwrap().to_string();

    let (upload_status, result) = upload_csv(&rt, &import, "Jerome,Kova\n").await;
    assert_eq!(upload_status, StatusCode::OK, "upload: {result}");

    let replay = rt
        .client
        .post(upload_url)
        .body("Again,Nope\n")
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::NOT_FOUND);
    rt.shutdown().await;
}

#[tokio::test]
async fn only_one_import_can_be_active_per_collection() {
    let rt = TestRuntime::boot().await;
    rt.install_manifest(&json!({
        "appId": "exclusive",
        "name": "exclusive",
        "version": "1.0.0",
        "dataContract": [
            { "entityName": "items", "fields": [{ "name": "name", "type": "text", "required": true }] },
            { "entityName": "offers", "fields": [{ "name": "name", "type": "text", "required": true }] }
        ]
    }))
    .await;

    let ((left_status, left), (right_status, right)) = tokio::join!(
        request_import(
            &rt,
            "exclusive",
            "items",
            json!({ "mode": "append", "columns": ["name"], "idempotencyKey": "left" }),
        ),
        request_import(
            &rt,
            "exclusive",
            "items",
            json!({ "mode": "append", "columns": ["name"], "idempotencyKey": "right" }),
        ),
    );
    let left_won = left_status == StatusCode::ACCEPTED && right_status == StatusCode::CONFLICT;
    let right_won = right_status == StatusCode::ACCEPTED && left_status == StatusCode::CONFLICT;
    assert!(left_won || right_won, "left: {left}; right: {right}");

    let (other_target_status, other_target) = request_import(
        &rt,
        "exclusive",
        "offers",
        json!({ "mode": "append", "columns": ["name"] }),
    )
    .await;
    assert_eq!(
        other_target_status,
        StatusCode::ACCEPTED,
        "other target: {other_target}"
    );
    rt.shutdown().await;
}

#[tokio::test]
async fn cancelled_import_can_be_retried_with_a_fresh_upload_token() {
    let rt = TestRuntime::boot().await;
    rt.install("retryable", "items").await;
    let (create_status, import) = request_import(
        &rt,
        "retryable",
        "items",
        json!({ "mode": "append", "columns": ["first_name", "last_name"] }),
    )
    .await;
    assert_eq!(create_status, StatusCode::ACCEPTED, "create: {import}");
    let old_upload_url = import["upload_url"].as_str().unwrap().to_string();
    let import_path = format!(
        "/api/v1/apps/retryable/collections/items/imports/{}",
        import["id"].as_str().unwrap()
    );

    let (cancel_status, cancel) = rt.delete_json(&import_path).await;
    assert_eq!(cancel_status, StatusCode::OK, "cancel: {cancel}");
    let (get_status, cancelled) = rt.get_json(&import_path).await;
    assert_eq!(get_status, StatusCode::OK, "cancelled: {cancelled}");
    assert_eq!(cancelled["status"], "cancelled");

    let old_token = rt
        .client
        .post(old_upload_url)
        .body("Stale,Token\n")
        .send()
        .await
        .unwrap();
    assert_eq!(old_token.status(), StatusCode::NOT_FOUND);

    let (retry_status, retried) = rt
        .post_json(&format!("{import_path}/retry"), &json!({}))
        .await;
    assert_eq!(retry_status, StatusCode::OK, "retry: {retried}");
    assert_eq!(retried["status"], "pending");
    assert_ne!(retried["upload_url"], import["upload_url"]);

    let (upload_status, completed) = upload_csv(&rt, &retried, "Fresh,Token\n").await;
    assert_eq!(upload_status, StatusCode::OK, "upload: {completed}");
    assert_eq!(completed["status"], "completed");
    rt.shutdown().await;
}

#[tokio::test]
async fn invalid_import_shapes_are_rejected_before_a_session_is_created() {
    let rt = TestRuntime::boot().await;
    rt.install_manifest(&json!({
        "appId": "validated",
        "name": "validated",
        "version": "1.0.0",
        "dataContract": [{
            "entityName": "contacts",
            "fields": [
                { "name": "email", "type": "text", "required": true },
                { "name": "name", "type": "text", "required": true }
            ],
            "indexes": [
                { "name": "uq_contacts_email", "columns": ["email"], "unique": true }
            ]
        }]
    }))
    .await;

    let cases = vec![
        ("empty columns", json!({ "mode": "append", "columns": [] })),
        (
            "duplicate columns",
            json!({ "mode": "append", "columns": ["name", "name"] }),
        ),
        (
            "unknown column",
            json!({ "mode": "append", "columns": ["missing"] }),
        ),
        (
            "upsert without conflicts",
            json!({ "mode": "upsert", "columns": ["email", "name"] }),
        ),
        (
            "append with conflicts",
            json!({ "mode": "append", "columns": ["email"], "conflictColumns": ["email"] }),
        ),
        (
            "conflict outside columns",
            json!({ "mode": "upsert", "columns": ["name"], "conflictColumns": ["email"] }),
        ),
        (
            "duplicate conflicts",
            json!({ "mode": "upsert", "columns": ["email"], "conflictColumns": ["email", "email"] }),
        ),
        (
            "non-unique conflict",
            json!({ "mode": "upsert", "columns": ["name"], "conflictColumns": ["name"] }),
        ),
        (
            "empty idempotency key",
            json!({ "mode": "append", "columns": ["name"], "idempotencyKey": "" }),
        ),
        (
            "oversized idempotency key",
            json!({ "mode": "append", "columns": ["name"], "idempotencyKey": "x".repeat(201) }),
        ),
    ];

    for (case, request) in cases {
        let (status, body) = request_import(&rt, "validated", "contacts", request).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "case '{case}': {body}");
    }
    rt.shutdown().await;
}

#[tokio::test]
async fn upsert_updates_conflicts_and_inserts_new_rows_atomically() {
    let rt = TestRuntime::boot().await;
    rt.install_manifest(&json!({
        "appId": "upserted",
        "name": "upserted",
        "version": "1.0.0",
        "dataContract": [{
            "entityName": "contacts",
            "fields": [
                { "name": "email", "type": "text", "required": true },
                { "name": "name", "type": "text", "required": true }
            ],
            "indexes": [
                { "name": "uq_contacts_email", "columns": ["email"], "unique": true }
            ]
        }]
    }))
    .await;
    rt.create(
        "upserted",
        "contacts",
        &json!({ "email": "jerome@kova.test", "name": "Ancien nom" }),
    )
    .await;

    let (create_status, import) = request_import(
        &rt,
        "upserted",
        "contacts",
        json!({
            "mode": "upsert",
            "columns": ["email", "name"],
            "conflictColumns": ["email"]
        }),
    )
    .await;
    assert_eq!(create_status, StatusCode::ACCEPTED, "create: {import}");

    let (upload_status, result) = upload_csv(
        &rt,
        &import,
        "jerome@kova.test,Jérôme de Kova\nnew@kova.test,Nouveau contact\n",
    )
    .await;
    assert_eq!(upload_status, StatusCode::OK, "upsert: {result}");

    let (_, rows) = rt
        .get_json("/api/v1/apps/upserted/collections/contacts?sort=email&order=asc")
        .await;
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 2);
    let jerome = rows
        .iter()
        .find(|row| row["email"] == "jerome@kova.test")
        .unwrap();
    assert_eq!(jerome["name"], "Jérôme de Kova");
    rt.shutdown().await;
}

#[tokio::test]
async fn failed_replace_keeps_the_published_collection_unchanged() {
    let rt = TestRuntime::boot().await;
    rt.install("atomic", "items").await;
    let original = rt
        .create(
            "atomic",
            "items",
            &json!({ "first_name": "Original", "last_name": "Record" }),
        )
        .await;
    let (create_status, import) = request_import(
        &rt,
        "atomic",
        "items",
        json!({ "mode": "replace", "columns": ["first_name", "last_name"] }),
    )
    .await;
    assert_eq!(create_status, StatusCode::ACCEPTED, "create: {import}");

    let (upload_status, failure) = upload_csv(&rt, &import, "Invalid,\\N\n").await;
    assert_eq!(
        upload_status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "failure: {failure}"
    );

    let (_, rows) = rt.get_json("/api/v1/apps/atomic/collections/items").await;
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], original["id"]);
    assert_eq!(rows[0]["first_name"], "Original");

    let (_, stored) = rt
        .get_json(&format!(
            "/api/v1/apps/atomic/collections/items/imports/{}",
            import["id"].as_str().unwrap()
        ))
        .await;
    assert_eq!(stored["status"], "failed");
    rt.shutdown().await;
}

#[tokio::test]
async fn empty_replace_requires_explicit_opt_in() {
    let rt = TestRuntime::boot().await;
    rt.install("empty_replace", "items").await;
    rt.create(
        "empty_replace",
        "items",
        &json!({ "first_name": "Original", "last_name": "Record" }),
    )
    .await;

    let (safe_create_status, safe_import) = request_import(
        &rt,
        "empty_replace",
        "items",
        json!({ "mode": "replace", "columns": ["first_name", "last_name"] }),
    )
    .await;
    assert_eq!(
        safe_create_status,
        StatusCode::ACCEPTED,
        "safe create: {safe_import}"
    );
    let (safe_upload_status, safe_result) = upload_csv(&rt, &safe_import, "").await;
    assert_eq!(
        safe_upload_status,
        StatusCode::BAD_REQUEST,
        "safe upload: {safe_result}"
    );
    let (_, preserved) = rt
        .get_json("/api/v1/apps/empty_replace/collections/items")
        .await;
    assert_eq!(preserved.as_array().unwrap().len(), 1);

    let (explicit_create_status, explicit_import) = request_import(
        &rt,
        "empty_replace",
        "items",
        json!({
            "mode": "replace",
            "columns": ["first_name", "last_name"],
            "allowEmpty": true
        }),
    )
    .await;
    assert_eq!(
        explicit_create_status,
        StatusCode::ACCEPTED,
        "explicit create: {explicit_import}"
    );
    let (explicit_upload_status, explicit_result) = upload_csv(&rt, &explicit_import, "").await;
    assert_eq!(
        explicit_upload_status,
        StatusCode::OK,
        "explicit upload: {explicit_result}"
    );
    let (_, emptied) = rt
        .get_json("/api/v1/apps/empty_replace/collections/items")
        .await;
    assert!(emptied.as_array().unwrap().is_empty());
    rt.shutdown().await;
}

#[tokio::test]
async fn import_modes_use_existing_collection_permissions() {
    let rt = TestRuntime::boot().await;
    rt.install_manifest(&json!({
        "appId": "governed",
        "name": "governed",
        "version": "1.0.0",
        "dataContract": [
            { "entityName": "appendable", "fields": [{ "name": "name", "type": "text", "required": true }] },
            { "entityName": "replaceable", "fields": [{ "name": "name", "type": "text", "required": true }] }
        ]
    }))
    .await;
    let token = rt.register_and_login("operator@test.local").await;
    sqlx::query(
        "INSERT INTO rootcx_system.rbac_roles (name, permissions)
         VALUES ('import_operator', ARRAY['app:governed:appendable.create', 'app:governed:replaceable.create'])",
    )
    .execute(rt.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO rootcx_system.rbac_assignments (user_id, role)
         SELECT id, 'import_operator' FROM rootcx_system.users WHERE email = 'operator@test.local'",
    )
    .execute(rt.pool())
    .await
    .unwrap();

    let (append_status, append) = rt
        .request_as(
            Method::POST,
            "/api/v1/apps/governed/collections/appendable/imports",
            &token,
            Some(&json!({ "mode": "append", "columns": ["name"] })),
        )
        .await;
    assert_eq!(append_status, StatusCode::ACCEPTED, "append: {append}");

    let (replace_status, replace) = rt
        .request_as(
            Method::POST,
            "/api/v1/apps/governed/collections/replaceable/imports",
            &token,
            Some(&json!({ "mode": "replace", "columns": ["name"] })),
        )
        .await;
    assert_eq!(replace_status, StatusCode::FORBIDDEN, "replace: {replace}");
    rt.shutdown().await;
}

#[tokio::test]
async fn collection_permission_is_rechecked_before_publication() {
    let rt = TestRuntime::boot().await;
    rt.install_manifest(&json!({
        "appId": "permission_recheck",
        "name": "permission_recheck",
        "version": "1.0.0",
        "dataContract": [{
            "entityName": "items",
            "fields": [{ "name": "name", "type": "text", "required": true }]
        }]
    }))
    .await;
    let token = rt.register_and_login("revoked@test.local").await;
    sqlx::query(
        "INSERT INTO rootcx_system.rbac_roles (name, permissions)
         VALUES ('revoked_import_operator', ARRAY['app:permission_recheck:items.create'])",
    )
    .execute(rt.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO rootcx_system.rbac_assignments (user_id, role)
         SELECT id, 'revoked_import_operator' FROM rootcx_system.users
         WHERE email = 'revoked@test.local'",
    )
    .execute(rt.pool())
    .await
    .unwrap();

    let (create_status, import) = rt
        .request_as(
            Method::POST,
            "/api/v1/apps/permission_recheck/collections/items/imports",
            &token,
            Some(&json!({ "mode": "append", "columns": ["name"] })),
        )
        .await;
    assert_eq!(create_status, StatusCode::ACCEPTED, "create: {import}");

    sqlx::query(
        "UPDATE rootcx_system.rbac_roles SET permissions = '{}' WHERE name = 'revoked_import_operator'",
    )
    .execute(rt.pool())
    .await
    .unwrap();
    let (upload_status, upload_result) = upload_csv(&rt, &import, "Kova\n").await;
    assert_eq!(
        upload_status,
        StatusCode::FORBIDDEN,
        "upload: {upload_result}"
    );
    let row_count: i64 = sqlx::query_scalar("SELECT count(*) FROM permission_recheck.items")
        .fetch_one(rt.pool())
        .await
        .unwrap();
    assert_eq!(row_count, 0);
    rt.shutdown().await;
}

#[tokio::test]
async fn source_storage_permission_is_rechecked_before_loading() {
    let rt = TestRuntime::boot().await;
    rt.install_manifest(&json!({
        "appId": "source_governed",
        "name": "source_governed",
        "version": "1.0.0",
        "dataContract": [{
            "entityName": "items",
            "fields": [{ "name": "name", "type": "text", "required": true }]
        }]
    }))
    .await;
    let (source_upload_status, file) = rt
        .upload(
            "/api/v1/apps/source_governed/storage/upload",
            "catalog.csv",
            "text/csv",
            b"Kova",
        )
        .await;
    assert_eq!(
        source_upload_status,
        StatusCode::CREATED,
        "source upload: {file}"
    );

    let token = rt.register_and_login("source-operator@test.local").await;
    sqlx::query(
        "INSERT INTO rootcx_system.rbac_roles (name, permissions)
         VALUES ('source_import_operator', ARRAY[
            'app:source_governed:items.create',
            'app:source_governed:storage.read'
         ])",
    )
    .execute(rt.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO rootcx_system.rbac_assignments (user_id, role)
         SELECT id, 'source_import_operator' FROM rootcx_system.users
         WHERE email = 'source-operator@test.local'",
    )
    .execute(rt.pool())
    .await
    .unwrap();

    let (create_status, import) = rt
        .request_as(
            Method::POST,
            "/api/v1/apps/source_governed/collections/items/imports",
            &token,
            Some(&json!({
                "mode": "append",
                "columns": ["name"],
                "sourceFileId": file["file_id"]
            })),
        )
        .await;
    assert_eq!(create_status, StatusCode::ACCEPTED, "create: {import}");

    sqlx::query(
        "UPDATE rootcx_system.rbac_roles
         SET permissions = ARRAY['app:source_governed:items.create']
         WHERE name = 'source_import_operator'",
    )
    .execute(rt.pool())
    .await
    .unwrap();
    let (upload_status, upload_result) = upload_csv(&rt, &import, "Kova\n").await;
    assert_eq!(
        upload_status,
        StatusCode::FORBIDDEN,
        "upload: {upload_result}"
    );

    let (get_status, stored) = rt
        .request_as(
            Method::GET,
            &format!(
                "/api/v1/apps/source_governed/collections/items/imports/{}",
                import["id"].as_str().unwrap()
            ),
            &token,
            None,
        )
        .await;
    assert_eq!(get_status, StatusCode::OK, "stored: {stored}");
    assert_eq!(stored["status"], "failed");
    rt.shutdown().await;
}

#[tokio::test]
#[ignore = "manual scale test: streams 600,000 rows through COPY"]
async fn streams_more_rows_than_the_original_kova_timeout() {
    let rt = TestRuntime::boot().await;
    rt.install_manifest(&json!({
        "appId": "scale_import",
        "name": "scale_import",
        "version": "1.0.0",
        "dataContract": [{
            "entityName": "items",
            "fields": [
                { "name": "external_id", "type": "text", "required": true },
                { "name": "label", "type": "text", "required": true }
            ]
        }]
    }))
    .await;
    let (create_status, import) = request_import(
        &rt,
        "scale_import",
        "items",
        json!({ "mode": "append", "columns": ["external_id", "label"] }),
    )
    .await;
    assert_eq!(create_status, StatusCode::ACCEPTED, "create: {import}");

    let chunks = stream::iter((0..60).map(|chunk| {
        let mut csv = String::with_capacity(280_000);
        for row in chunk * 10_000..(chunk + 1) * 10_000 {
            use std::fmt::Write as _;
            writeln!(csv, "ASAMCO-{row},Article ASAMCO {row}").unwrap();
        }
        Ok::<_, std::io::Error>(csv)
    }));
    let response = rt
        .client
        .post(import["upload_url"].as_str().unwrap())
        .header("content-type", "text/csv")
        .body(reqwest::Body::wrap_stream(chunks))
        .send()
        .await
        .unwrap();
    let status = response.status();
    let result: Value = response.json().await.unwrap();
    assert_eq!(status, StatusCode::OK, "scale import: {result}");
    assert_eq!(result["rows_loaded"], 600_000);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM scale_import.items")
        .fetch_one(rt.pool())
        .await
        .unwrap();
    assert_eq!(count, 600_000);
    rt.shutdown().await;
}
