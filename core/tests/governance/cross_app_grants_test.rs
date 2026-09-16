//! Black-box contract tests for Core-managed cross-application collection grants.
//!
//! These tests intentionally exercise the public grant API plus the shared
//! authorization Module.  They require the same PostgreSQL test container as
//! the other Core integration tests.

use crate::harness;

use reqwest::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

fn provider_manifest(app_id: &str) -> Value {
    json!({
        "appId": app_id,
        "name": app_id,
        "version": "1.0.0",
        "dataContract": [{
            "entityName": "records",
            "fields": [
                { "name": "name", "type": "text" },
                { "name": "status", "type": "text" },
                { "name": "secret", "type": "text", "sensitive": true }
            ]
        }]
    })
}

async fn installation(rt: &harness::TestRuntime, app_id: &str) -> (Uuid, i64) {
    sqlx::query_as(
        "SELECT id, generation FROM rootcx_system.app_installations
          WHERE app_id = $1 AND active = TRUE",
    )
    .bind(app_id)
    .fetch_one(rt.pool())
    .await
    .unwrap()
}

#[tokio::test]
async fn grant_lifecycle_freezes_scope_and_records_history() {
    let rt = harness::TestRuntime::boot().await;
    rt.install("consumer", "events").await;
    rt.install_manifest(&provider_manifest("provider")).await;

    let (status, created) = rt
        .post_json(
            "/api/v1/cross-app/grants",
            &json!({
                "consumerApp": "consumer",
                "providerApp": "provider",
                "entity": "records",
                "actions": ["list"],
                "fields": ["name"],
                "reason": "reporting"
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["status"], "pending");
    let grant_id = created["id"].as_str().unwrap();
    let fields = created["fieldSnapshot"].as_array().unwrap();
    assert!(fields.iter().any(|field| field == "name"));
    assert!(!fields.iter().any(|field| field == "secret"));

    let (status, history) = rt
        .get_json(&format!("/api/v1/cross-app/grants/{grant_id}/audit"))
        .await;
    assert_eq!(status, StatusCode::OK, "{history}");
    assert_eq!(history.as_array().map(Vec::len), Some(1));
    assert_eq!(history[0]["operation"], "created");
    assert_eq!(history[0]["beforeState"], Value::Null);
    assert_eq!(
        history[0]["afterState"]["fieldSnapshot"],
        created["fieldSnapshot"]
    );

    let (status, active) = rt
        .post_json(
            &format!("/api/v1/cross-app/grants/{grant_id}/approve"),
            &json!({"reason": "provider approved"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{active}");
    assert_eq!(active["status"], "active");
    assert_eq!(active["version"].as_i64(), Some(2));

    let allowed = rootcx_core::governance::cross_app::authorize_cross_app_read(
        rt.pool(),
        "consumer",
        "provider",
        "records",
        "list",
        &["name".into()],
    )
    .await;
    assert!(
        allowed.is_ok(),
        "approved field should be authorized: {allowed:?}"
    );
    let forbidden = rootcx_core::governance::cross_app::authorize_cross_app_read(
        rt.pool(),
        "consumer",
        "provider",
        "records",
        "list",
        &["secret".into()],
    )
    .await;
    assert!(
        forbidden.is_err(),
        "sensitive/unapproved field must be denied"
    );

    let (status, disabled) = rt
        .post_json(
            &format!("/api/v1/cross-app/grants/{grant_id}/disable"),
            &json!({"reason": "temporarily paused"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{disabled}");
    assert_eq!(disabled["status"], "disabled");
    let denied_while_disabled =
        rootcx_core::governance::cross_app::authorize_cross_app_read(
            rt.pool(),
            "consumer",
            "provider",
            "records",
            "list",
            &["name".into()],
        )
        .await;
    assert!(
        denied_while_disabled.is_err(),
        "disabled grants must deny later authorizations"
    );

    let (status, reenabled) = rt
        .post_json(
            &format!("/api/v1/cross-app/grants/{grant_id}/enable"),
            &json!({"reason": "back in service"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{reenabled}");
    assert_eq!(reenabled["status"], "active");

    let (status, revoked) = rt
        .post_json(
            &format!("/api/v1/cross-app/grants/{grant_id}/revoke"),
            &json!({"reason": "no longer needed"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{revoked}");
    assert_eq!(revoked["status"], "revoked");
    let denied = rootcx_core::governance::cross_app::authorize_cross_app_read(
        rt.pool(),
        "consumer",
        "provider",
        "records",
        "list",
        &["name".into()],
    )
    .await;
    assert!(denied.is_err(), "revocation must cut later authorizations");

    let (status, disabled_again) = rt
        .post_json(
            &format!("/api/v1/cross-app/grants/{grant_id}/disable"),
            &json!({"reason": "must remain terminated"}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{disabled_again}");
    let (status, enabled_again) = rt
        .post_json(
            &format!("/api/v1/cross-app/grants/{grant_id}/enable"),
            &json!({"reason": "must remain terminated"}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{enabled_again}");

    let (status, history) = rt
        .get_json(&format!("/api/v1/cross-app/grants/{grant_id}/audit"))
        .await;
    assert_eq!(status, StatusCode::OK, "{history}");
    assert_eq!(history.as_array().map(Vec::len), Some(5));
    assert_eq!(history[0]["operation"], "revoked");
    assert_eq!(history[1]["operation"], "enabled");
    assert_eq!(history[2]["operation"], "disabled");
    assert_eq!(history[3]["operation"], "approved");
    assert_eq!(history[4]["operation"], "created");
    assert_eq!(
        history[0]["beforeState"]["fieldSnapshot"],
        created["fieldSnapshot"]
    );

    rt.shutdown().await;
}

#[tokio::test]
async fn uninstall_revokes_generation_and_reinstall_does_not_restore_grant() {
    let rt = harness::TestRuntime::boot().await;
    rt.install("consumer", "events").await;
    rt.install_manifest(&provider_manifest("provider")).await;
    let (old_provider_id, old_generation) = installation(&rt, "provider").await;

    let (status, created) = rt
        .post_json(
            "/api/v1/cross-app/grants",
            &json!({
                "consumerApp": "consumer",
                "providerApp": "provider",
                "entity": "records",
                "fields": ["name"]
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let grant_id = created["id"].as_str().unwrap();
    let (status, approved) = rt
        .post_json(
            &format!("/api/v1/cross-app/grants/{grant_id}/approve"),
            &json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{approved}");

    assert_eq!(rt.delete("/api/v1/apps/provider").await, StatusCode::OK);
    let (status, revoked) = rt
        .get_json(&format!("/api/v1/cross-app/grants/{grant_id}"))
        .await;
    assert_eq!(status, StatusCode::OK, "{revoked}");
    assert_eq!(revoked["status"], "revoked");

    rt.install_manifest(&provider_manifest("provider")).await;
    let (new_provider_id, new_generation) = installation(&rt, "provider").await;
    assert_ne!(new_provider_id, old_provider_id);
    assert_eq!(new_generation, old_generation + 1);
    let denied = rootcx_core::governance::cross_app::authorize_cross_app_read(
        rt.pool(),
        "consumer",
        "provider",
        "records",
        "list",
        &["name".into()],
    )
    .await;
    assert!(
        denied.is_err(),
        "reinstall must not revive a grant from the old generation"
    );

    let (status, history) = rt
        .get_json(&format!("/api/v1/cross-app/grants/{grant_id}/audit"))
        .await;
    assert_eq!(status, StatusCode::OK, "{history}");
    assert_eq!(history[0]["operation"], "auto_revoked");
    assert_eq!(history[0]["afterState"]["status"], "revoked");

    rt.shutdown().await;
}

#[tokio::test]
async fn provider_ownership_remains_authoritative_for_a_cross_app_read() {
    let rt = harness::TestRuntime::boot().await;
    rt.install("consumer", "events").await;
    rt.install_manifest(&json!({
        "appId": "provider",
        "name": "provider",
        "version": "1.0.0",
        "dataContract": [{
            "entityName": "records",
            "fields": [
                { "name": "user_id", "type": "uuid", "owner": true },
                { "name": "name", "type": "text" }
            ]
        }]
    }))
    .await;
    let _ = rt.register_and_login("plain@t.local").await;
    let plain: Uuid =
        sqlx::query_scalar("SELECT id FROM rootcx_system.users WHERE email = 'plain@t.local'")
            .fetch_one(rt.pool())
            .await
            .unwrap();
    let other = Uuid::new_v4();
    sqlx::query("INSERT INTO provider.records (user_id, name) VALUES ($1, 'mine'), ($2, 'other')")
        .bind(plain)
        .bind(other)
        .execute(rt.pool())
        .await
        .unwrap();

    let (status, created) = rt
        .post_json(
            "/api/v1/cross-app/grants",
            &json!({
                "consumerApp": "consumer",
                "providerApp": "provider",
                "entity": "records",
                "fields": ["name"]
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let grant_id = created["id"].as_str().unwrap();
    let (status, approved) = rt
        .post_json(
            &format!("/api/v1/cross-app/grants/{grant_id}/approve"),
            &json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{approved}");

    let authority = rootcx_core::governance::cross_app::authorize_cross_app_read(
        rt.pool(),
        "consumer",
        "provider",
        "records",
        "list",
        &["name".into()],
    )
    .await
    .unwrap();
    let state = rootcx_core::governance::enforcement::ContextState {
        user_id: Some(plain),
        is_delegated: false,
        effective_perms: vec![],
        connection_id: None,
        audit_actor_id: Some(plain),
        audit_delegator_id: None,
    };
    let mut tx = rootcx_core::governance::enforcement::begin_app_tx_with_invocation_and_cross_app(
        rt.pool(),
        "provider",
        &state,
        &rootcx_core::governance::enforcement::InvocationContext::default(),
        Some(plain),
        None,
        "cross_app_test",
        rootcx_core::governance::enforcement::TIMEOUT_INTERACTIVE_MS,
        Some(&authority),
    )
    .await
    .unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM provider.records")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        count, 1,
        "provider ownership must still filter cross-app rows"
    );

    rt.shutdown().await;
}

#[tokio::test]
async fn worker_remote_reads_enforce_scope_pagination_revocation_and_audit() {
    let rt = harness::TestRuntime::boot().await;
    rt.install("consumer", "events").await;
    let mut manifest = provider_manifest("provider");
    manifest["dataContract"][0]["fields"]
        .as_array_mut().unwrap()
        .extend([
            json!({"name": "limit", "type": "number"}),
            json!({"name": "data", "type": "json"}),
        ]);
    rt.install_manifest(&manifest).await;
    sqlx::query(
        "INSERT INTO provider.records (name, status, secret, \"limit\", data)
         VALUES ('Ada', 'published', 'hidden', 10, '[1,2,3]'),
                ('Grace', 'published', 'other', 10, '[]')",
    ).execute(rt.pool()).await.unwrap();

    let backend = br#"
serve({ rpc: {
  remote: async (params, _caller, ctx) => {
    try {
      const collection = ctx.remote("provider").collection("records");
      return { result: await collection[params.op](params.query) };
    } catch (error) {
      return { denied: error.message };
    }
  },
} });
"#;
    let (status, deployed) = rt.deploy(
        "consumer", &harness::make_tar_gz(&[("index.ts", backend)]),
    ).await;
    assert_eq!(status, StatusCode::OK, "{deployed}");
    let token = rt.register_and_login("consumer@test.local").await;
    sqlx::query(
        "INSERT INTO rootcx_system.rbac_roles (name, permissions)
         VALUES ('consumer_only', ARRAY['app:consumer:invoke'])",
    ).execute(rt.pool()).await.unwrap();
    sqlx::query(
        "INSERT INTO rootcx_system.rbac_assignments (user_id, role)
         SELECT id, 'consumer_only' FROM rootcx_system.users WHERE email = 'consumer@test.local'",
    ).execute(rt.pool()).await.unwrap();

    let call = |op: &str, query: Value| json!({
        "method": "remote", "params": {"op": op, "query": query},
    });
    let (_, denied) = rt.request_as(
        reqwest::Method::POST, "/api/v1/apps/consumer/rpc", &token,
        Some(&call("find", json!({}))),
    ).await;
    assert!(denied["denied"].is_string(), "{denied}");

    let (status, created) = rt.post_json("/api/v1/cross-app/grants", &json!({
        "consumerApp": "consumer", "providerApp": "provider",
        "entity": "records", "fields": ["name"],
    })).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().unwrap();
    let (status, approved) = rt.post_json(
        &format!("/api/v1/cross-app/grants/{id}/approve"), &json!({}),
    ).await;
    assert_eq!(status, StatusCode::OK, "{approved}");

    for (query, expected_len, expected_total) in [
        (json!({"orderBy": "name", "order": "asc", "limit": 1}), 1, 2),
        (json!({"offset": 2}), 0, 2),
        (json!({"where": {"name": "Ada"}, "offset": 1}), 0, 1),
    ] {
        let (status, body) = rt.request_as(
            reqwest::Method::POST, "/api/v1/apps/consumer/rpc", &token,
            Some(&call("findPage", query)),
        ).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["result"]["total"], expected_total, "{body}");
        let rows = body["result"]["data"].as_array().expect("page data");
        assert_eq!(rows.len(), expected_len, "{body}");
        for row in rows {
            assert!(row.get("secret").is_none(), "{row}");
            assert!(row.get("status").is_none(), "{row}");
        }
    }
    let (status, body) = rt.request_as(
        reqwest::Method::POST, "/api/v1/apps/consumer/rpc", &token,
        Some(&call("find", json!({"name": "Ada"}))),
    ).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rows = body["result"].as_array().expect("find returns an equality array");
    assert_eq!(rows.len(), 1, "{body}");
    assert_eq!(rows[0]["name"], "Ada", "{body}");
    assert!(rows[0].get("secret").is_none(), "{body}");
    assert!(rows[0].get("status").is_none(), "{body}");
    let (_, body) = rt.request_as(
        reqwest::Method::POST, "/api/v1/apps/consumer/rpc", &token,
        Some(&call("findOne", json!({"name": "Ada"}))),
    ).await;
    assert_eq!(body["result"]["name"], "Ada", "{body}");
    assert!(body["result"].get("secret").is_none(), "{body}");
    assert!(body["result"].get("data").is_none(), "{body}");
    let read_count: i64 = sqlx::query_scalar(
        "SELECT row_count FROM rootcx_system.cross_app_read_audit
         WHERE consumer_app = 'consumer' AND action = 'read' AND outcome = 'success'",
    ).fetch_one(rt.pool()).await.unwrap();
    assert_eq!(read_count, 1, "a record's JSON data field is not a result page");

    for (op, query) in [
        ("find", json!({"limit": 10, "secret": "hidden"})),
        ("findOne", json!({"limit": 10, "secret": "hidden"})),
        ("findPage", json!({"where": {"secret": "hidden"}})),
        ("findPage", json!({"orderBy": "secret"})),
        ("create", json!({"name": "read_grant_cannot_write"})),
    ] {
        let (_, body) = rt.request_as(
            reqwest::Method::POST, "/api/v1/apps/consumer/rpc", &token,
            Some(&call(op, query)),
        ).await;
        assert!(body["denied"].is_string(), "{body}");
        assert!(body.get("result").is_none(), "{body}");
    }

    let (status, revoked) = rt.post_json(
        &format!("/api/v1/cross-app/grants/{id}/revoke"), &json!({}),
    ).await;
    assert_eq!(status, StatusCode::OK, "{revoked}");
    let (_, body) = rt.request_as(
        reqwest::Method::POST, "/api/v1/apps/consumer/rpc", &token,
        Some(&call("find", json!({}))),
    ).await;
    assert!(body["denied"].is_string(), "{body}");

    let outcomes: Vec<(String, i64)> = sqlx::query_as(
        "SELECT outcome, count(*) FROM rootcx_system.cross_app_read_audit
         WHERE consumer_app = 'consumer' GROUP BY outcome ORDER BY outcome",
    ).fetch_all(rt.pool()).await.unwrap();
    assert_eq!(outcomes, vec![
        ("denied".into(), 7), ("started".into(), 5), ("success".into(), 5),
    ]);
    rt.shutdown().await;
}
