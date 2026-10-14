mod harness;
use harness::TestRuntime;
use reqwest::{Method, StatusCode};
use serde_json::json;

async fn setup_integration(rt: &TestRuntime) {
    rt.install_manifest(&json!({
        "appId": "test_integ",
        "name": "Test Integration",
        "version": "1.0.0",
        "type": "integration",
        "configSchema": {
            "type": "object",
            "properties": {
                "apiKey": { "type": "string", "platformSecret": "TEST_INTEG_KEY" }
            }
        },
        "dataContract": []
    })).await;
}

async fn create_connection(rt: &TestRuntime, token: &str, integration_id: &str, label: &str) -> String {
    let (s, body) = rt.request_as(
        Method::POST,
        &format!("/api/v1/integrations/{integration_id}/auth/credentials"),
        token,
        Some(&json!({"credentials": {"apiKey": "test"}, "label": label})),
    ).await;
    assert_eq!(s, StatusCode::OK, "submit_credentials failed: {body}");
    body["connectionId"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn connection_lifecycle_crud() {
    let rt = TestRuntime::boot().await;
    setup_integration(&rt).await;

    // Create two connections via submit_credentials
    let conn1 = create_connection(&rt, &rt.token, "test_integ", "Account A").await;
    let conn2 = create_connection(&rt, &rt.token, "test_integ", "Account B").await;
    assert_ne!(conn1, conn2);

    // List connections
    let (s, body) = rt.get_json("/api/v1/integrations/test_integ/connections").await;
    assert_eq!(s, StatusCode::OK);
    let connections = body.as_array().unwrap();
    assert!(connections.len() >= 2, "expected at least 2 connections, got {}", connections.len());

    let labels: Vec<&str> = connections.iter().filter_map(|c| c["label"].as_str()).collect();
    assert!(labels.contains(&"Account A"), "missing Account A in {labels:?}");
    assert!(labels.contains(&"Account B"), "missing Account B in {labels:?}");

    // Update label
    let (s, _) = rt.patch_json(
        &format!("/api/v1/integrations/test_integ/connections/{conn1}"),
        &json!({"label": "Primary"}),
    ).await;
    assert_eq!(s, StatusCode::OK);

    // Verify update
    let (_, body) = rt.get_json("/api/v1/integrations/test_integ/connections").await;
    let updated = body.as_array().unwrap().iter().find(|c| c["id"].as_str() == Some(&conn1)).unwrap();
    assert_eq!(updated["label"].as_str().unwrap(), "Primary");

    // Delete connection
    let (s, _) = rt.delete_json(&format!("/api/v1/integrations/test_integ/connections/{conn1}")).await;
    assert_eq!(s, StatusCode::OK);

    // Verify deletion
    let (_, body) = rt.get_json("/api/v1/integrations/test_integ/connections").await;
    let ids: Vec<&str> = body.as_array().unwrap().iter().filter_map(|c| c["id"].as_str()).collect();
    assert!(!ids.contains(&conn1.as_str()), "conn1 should be deleted");

    rt.shutdown().await;
}

#[tokio::test]
async fn connection_ownership_enforced() {
    let rt = TestRuntime::boot().await;
    setup_integration(&rt).await;

    // User A creates a connection
    let conn_id = create_connection(&rt, &rt.token, "test_integ", "User A account").await;

    // User B tries to delete it
    let user_b_token = rt.create_user("userb@test.local").await;
    let (s, _) = rt.request_as(
        Method::DELETE,
        &format!("/api/v1/integrations/test_integ/connections/{conn_id}"),
        &user_b_token,
        None,
    ).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "user B should not delete user A's connection");

    // User B tries to update it
    let (s, _) = rt.request_as(
        Method::PATCH,
        &format!("/api/v1/integrations/test_integ/connections/{conn_id}"),
        &user_b_token,
        Some(&json!({"label": "hacked"})),
    ).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "user B should not update user A's connection");

    rt.shutdown().await;
}

#[tokio::test]
async fn duplicate_label_reuses_connection() {
    let rt = TestRuntime::boot().await;
    setup_integration(&rt).await;

    let conn1 = create_connection(&rt, &rt.token, "test_integ", "same@example.com").await;
    let conn2 = create_connection(&rt, &rt.token, "test_integ", "same@example.com").await;
    assert_eq!(conn1, conn2, "same label should return same connection_id");

    let conn3 = create_connection(&rt, &rt.token, "test_integ", "different@example.com").await;
    assert_ne!(conn1, conn3, "different label should create new connection");

    let (_, body) = rt.get_json("/api/v1/integrations/test_integ/connections").await;
    let connections = body.as_array().unwrap();
    assert_eq!(connections.len(), 2, "should have exactly 2 connections, not 3");

    rt.shutdown().await;
}

#[tokio::test]
async fn app_binding_with_connection_selection() {
    let rt = TestRuntime::boot().await;
    setup_integration(&rt).await;

    // Install a consumer app
    rt.install("consumer_app", "contacts").await;

    // Create two connections
    let conn1 = create_connection(&rt, &rt.token, "test_integ", "work@example.com").await;
    let conn2 = create_connection(&rt, &rt.token, "test_integ", "personal@example.com").await;

    // Bind app to integration with specific connection
    let (s, _) = rt.post_json(
        "/api/v1/apps/consumer_app/integrations",
        &json!({"integrationId": "test_integ", "connectionId": conn2}),
    ).await;
    assert_eq!(s, StatusCode::OK);

    // List bindings - verify connection is set
    let (s, body) = rt.get_json("/api/v1/apps/consumer_app/integrations").await;
    assert_eq!(s, StatusCode::OK);
    let bindings = body.as_array().unwrap();
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0]["integrationId"].as_str().unwrap(), "test_integ");
    assert_eq!(bindings[0]["connectionId"].as_str().unwrap(), &conn2);

    // Switch to different connection
    let (s, _) = rt.post_json(
        "/api/v1/apps/consumer_app/integrations",
        &json!({"integrationId": "test_integ", "connectionId": conn1}),
    ).await;
    assert_eq!(s, StatusCode::OK);

    let (_, body) = rt.get_json("/api/v1/apps/consumer_app/integrations").await;
    assert_eq!(body.as_array().unwrap()[0]["connectionId"].as_str().unwrap(), &conn1);

    // Unbind
    let s = rt.delete("/api/v1/apps/consumer_app/integrations/test_integ").await;
    assert_eq!(s, StatusCode::OK);

    let (_, body) = rt.get_json("/api/v1/apps/consumer_app/integrations").await;
    assert!(body.as_array().unwrap().is_empty());

    rt.shutdown().await;
}

#[tokio::test]
async fn status_excludes_dead_connections() {
    let rt = TestRuntime::boot().await;
    setup_integration(&rt).await;

    let _live = create_connection(&rt, &rt.token, "test_integ", "live@example.com").await;
    let dead = create_connection(&rt, &rt.token, "test_integ", "dead@example.com").await;

    // Simulate the provider having rejected this connection's credentials
    // (what flag_if_auth_failed does on INSUFFICIENT_PERMISSIONS).
    sqlx::query("UPDATE rootcx_system.integration_connections SET status = 'dead' WHERE id = $1")
        .bind(&dead)
        .execute(rt.pool()).await.unwrap();

    let (s, body) = rt.get_json("/api/v1/integrations/test_integ/auth").await;
    assert_eq!(s, StatusCode::OK);
    // The silent-failure guard: a dead grant must not be counted as connected.
    assert_eq!(body["connected"], json!(true), "one live connection remains: {body}");
    assert_eq!(body["connectionCount"], json!(1), "dead connection excluded from live count: {body}");
    assert_eq!(body["deadCount"], json!(1), "dead connection surfaced separately: {body}");

    rt.shutdown().await;
}

#[tokio::test]
async fn reconnecting_revives_a_dead_connection() {
    let rt = TestRuntime::boot().await;
    setup_integration(&rt).await;

    let conn = create_connection(&rt, &rt.token, "test_integ", "mailbox@example.com").await;
    sqlx::query("UPDATE rootcx_system.integration_connections SET status = 'dead', last_error = 'invalid_grant' WHERE id = $1")
        .bind(&conn)
        .execute(rt.pool()).await.unwrap();

    // Reconnecting the same mailbox (same label) reuses the row and clears the dead flag.
    let reconnected = create_connection(&rt, &rt.token, "test_integ", "mailbox@example.com").await;
    assert_eq!(reconnected, conn, "same label reuses the connection");

    let (_, body) = rt.get_json("/api/v1/integrations/test_integ/auth").await;
    assert_eq!(body["connected"], json!(true), "connection revived: {body}");
    assert_eq!(body["deadCount"], json!(0), "dead flag cleared on reconnect: {body}");

    rt.shutdown().await;
}

#[tokio::test]
async fn app_wide_binding_requires_manage_permission() {
    let rt = TestRuntime::boot().await;
    setup_integration(&rt).await;
    rt.install("shared_app", "items").await;

    // A non-admin user with their own connection, holding no elevated permission.
    let user_b_token = rt.create_user("userb@test.local").await;
    let conn_b = create_connection(&rt, &user_b_token, "test_integ", "userb@example.com").await;

    // App-wide (shared) binding is refused without integration:<id>:manage.
    let app_wide = json!({"integrationId": "test_integ", "connectionId": conn_b});
    let (s, body) = rt.request_as(
        Method::POST, "/api/v1/apps/shared_app/integrations", &user_b_token, Some(&app_wide),
    ).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "app-wide bind without manage perm must be denied: {body}");

    // A user-scoped binding of their OWN connection needs no elevated permission.
    let user_scoped = json!({"integrationId": "test_integ", "connectionId": conn_b, "scope": "user"});
    let (s, body) = rt.request_as(
        Method::POST, "/api/v1/apps/shared_app/integrations", &user_b_token, Some(&user_scoped),
    ).await;
    assert_eq!(s, StatusCode::OK, "user-scoped self-bind needs no elevated perm: {body}");

    rt.shutdown().await;
}

#[tokio::test]
async fn provider_configs_are_isolated_per_instance() {
    let rt = TestRuntime::boot().await;
    setup_integration(&rt).await;

    // Two named provider configs = two OAuth clients for the same integration.
    let (s, a) = rt.post_json("/api/v1/integrations/test_integ/configs",
        &json!({"label": "Project A", "credentials": {"apiKey": "key-a"}})).await;
    assert_eq!(s, StatusCode::OK, "create A: {a}");
    let cfg_a = a["id"].as_str().unwrap().to_string();
    let (s, b) = rt.post_json("/api/v1/integrations/test_integ/configs",
        &json!({"label": "Project B", "credentials": {"apiKey": "key-b"}})).await;
    assert_eq!(s, StatusCode::OK, "create B: {b}");
    assert_ne!(cfg_a, b["id"].as_str().unwrap(), "each config has its own id");

    // Both listed and configured (credentials live at their own scope, not shared).
    let (s, list) = rt.get_json("/api/v1/integrations/test_integ/configs").await;
    assert_eq!(s, StatusCode::OK);
    let arr = list.as_array().unwrap();
    assert_eq!(arr.len(), 2, "two configs: {list}");
    assert!(arr.iter().all(|c| c["configured"] == json!(true)), "both configured: {list}");

    // Delete one (no connection references it) — the other survives.
    let s = rt.delete(&format!("/api/v1/integrations/test_integ/configs/{cfg_a}")).await;
    assert_eq!(s, StatusCode::OK);
    let (_, list) = rt.get_json("/api/v1/integrations/test_integ/configs").await;
    assert_eq!(list.as_array().unwrap().len(), 1, "one config remains after delete");

    rt.shutdown().await;
}

#[tokio::test]
async fn bind_rejects_connection_not_owned_by_caller() {
    let rt = TestRuntime::boot().await;
    setup_integration(&rt).await;
    rt.install("my_app", "items").await;

    // User A creates a connection
    let conn_a = create_connection(&rt, &rt.token, "test_integ", "admin account").await;

    // User B tries to bind their app using User A's connection
    let user_b_token = rt.create_user("userb@test.local").await;
    let (s, body) = rt.request_as(
        Method::POST,
        "/api/v1/apps/my_app/integrations",
        &user_b_token,
        Some(&json!({"integrationId": "test_integ", "connectionId": conn_a})),
    ).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "should reject unowned connection: {body}");

    rt.shutdown().await;
}

/// A worker's process environment must never carry connection credentials.
///
/// `get_env_for_app` returns every key under the app id, and an integration's
/// connection credentials are stored under that same id. Left unfiltered, one
/// integration worker holds every user's credential for the life of the process,
/// long after the call that needed one has ended.
#[tokio::test]
async fn connection_credentials_stay_out_of_worker_environment() {
    let rt = TestRuntime::boot().await;
    setup_integration(&rt).await;
    let conn = create_connection(&rt, &rt.token, "test_integ", "Account A").await;

    let secrets = rt.runtime.secret_manager();
    secrets.set(rt.pool(), "test_integ", "PLAIN_KEY", "visible").await.unwrap();

    let env = secrets.get_env_for_app(rt.pool(), "test_integ").await.unwrap();

    let leaked: Vec<_> = env.keys().filter(|k| k.starts_with("_conn.")).collect();
    assert!(leaked.is_empty(), "connection credentials reached the worker environment: {leaked:?}");
    assert!(env.contains_key("PLAIN_KEY"), "ordinary app secrets must still reach the worker");

    // Withholding must not mean discarding: the core still resolves it per call.
    let stored = secrets.get(rt.pool(), "test_integ", &format!("_conn.{conn}")).await.unwrap();
    assert!(stored.is_some(), "the credential must remain stored for the core to resolve");

    rt.shutdown().await;
}

#[tokio::test]
async fn explicit_account_selection_cannot_borrow_another_users_connection_even_as_admin() {
    let rt = TestRuntime::boot().await;
    setup_integration(&rt).await;
    let other = rt.create_user("plugin-owner@test.local").await;
    let connection = create_connection(&rt, &other, "test_integ", "Other account").await;
    let response = rt.client.post(rt.url("/api/v1/integrations/test_integ/actions/verify_connection"))
        .bearer_auth(&rt.token).header("x-integration-connection", &connection).json(&json!({})).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    rt.shutdown().await;
}

#[tokio::test]
async fn a_catalog_plugin_starts_on_demand_and_rejects_unverified_credentials() {
    let rt = TestRuntime::boot().await;
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/integrations/stripe");
    let dest = rt.runtime.resources_dir().join("integrations/stripe");
    std::fs::create_dir_all(&dest).unwrap();
    for file in ["index.ts", "manifest.json", "plugin-lock.json", "package.json"] {
        std::fs::copy(src.join(file), dest.join(file)).unwrap();
    }
    let (status, body) = rt.post_json("/api/v1/integrations/catalog/stripe/deploy", &json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // This crosses the actual Core -> worker RPC boundary. Empty credentials
    // must fail locally, without sending a request to Stripe or storing a connection.
    let (status, _) = rt.post_json("/api/v1/integrations/stripe/auth/credentials", &json!({ "credentials": {} })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (_, connections) = rt.get_json("/api/v1/integrations/stripe/connections").await;
    assert_eq!(connections, json!([]));
    rt.shutdown().await;
}

#[tokio::test]
async fn oauth_receipts_are_private_expiring_and_only_expose_successful_connections() {
    use sha2::{Digest, Sha256};
    let rt = TestRuntime::boot().await;
    let (_, me) = rt.get_json("/api/v1/auth/me").await;
    let owner = me["id"].as_str().unwrap();
    let other = rt.create_user("receipt-other@test.local").await;
    for status in ["pending", "failed", "cancelled", "connected"] {
        let attempt = uuid::Uuid::new_v4().to_string();
        let hash = hex::encode(Sha256::digest(attempt.as_bytes()));
        sqlx::query("INSERT INTO rootcx_system.integration_oauth_attempts (id, integration_id, user_id, connection_id, status) VALUES ($1, 'google_drive', $2, 'receipt-connection', $3)")
            .bind(&hash).bind(owner).bind(status).execute(rt.runtime.pool()).await.unwrap();
        let path = format!("/api/v1/integrations/google_drive/auth/attempts/{attempt}");
        let (code, body) = rt.get_json(&path).await;
        assert_eq!(code, StatusCode::OK, "{body}");
        assert_eq!(body["status"], status);
        assert_eq!(body["connectionId"], if status == "connected" { json!("receipt-connection") } else { serde_json::Value::Null });
        let (code, _) = rt.request_as(Method::GET, &path, &other, None).await;
        assert_eq!(code, StatusCode::NOT_FOUND);
        let (code, _) = rt.get_json(&path.replace("google_drive", "notion")).await;
        assert_eq!(code, StatusCode::NOT_FOUND);
        sqlx::query("UPDATE rootcx_system.integration_oauth_attempts SET expires_at = now() - interval '1 second' WHERE id = $1")
            .bind(&hash).execute(rt.runtime.pool()).await.unwrap();
        let (code, _) = rt.get_json(&path).await;
        assert_eq!(code, StatusCode::NOT_FOUND);
    }
    rt.shutdown().await;
}

#[tokio::test]
async fn managed_google_configuration_and_cancelled_callback_stay_server_owned() {
    if std::env::var("GOOGLE_MANAGED_TEST_CHILD").as_deref() != Ok("true") {
        let output = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "managed_google_configuration_and_cancelled_callback_stay_server_owned", "--nocapture"])
            .env("GOOGLE_MANAGED_TEST_CHILD", "true")
            .env("ROOTCX_MANAGED_GOOGLE_OAUTH", "true")
            .env("ROOTCX_GOOGLE_CLIENT_ID", "fixture-google-client")
            .env("ROOTCX_GOOGLE_CLIENT_SECRET", "fixture-google-secret")
            .output().await.unwrap();
        assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        return;
    }
    let rt = TestRuntime::boot().await;
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/integrations/google_drive");
    let dest = rt.runtime.resources_dir().join("integrations/google_drive");
    std::fs::create_dir_all(&dest).unwrap();
    for file in ["index.ts", "manifest.json", "plugin-lock.json", "package.json"] {
        std::fs::copy(src.join(file), dest.join(file)).unwrap();
    }
    let (code, body) = rt.post_json("/api/v1/integrations/catalog/google_drive/deploy", &json!({})).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    let (code, _) = rt.request_as(Method::PUT, "/api/v1/integrations/google_drive/config", &rt.token, Some(&json!({"clientSecret":"tenant-override"}))).await;
    assert_eq!(code, StatusCode::FORBIDDEN);
    let (code, _) = rt.post_json("/api/v1/integrations/google_drive/configs", &json!({"label":"alternate", "credentials":{}})).await;
    assert_eq!(code, StatusCode::FORBIDDEN);
    let (_, integrations) = rt.get_json("/api/v1/integrations").await;
    let drive = integrations.as_array().unwrap().iter().find(|i| i["id"] == "google_drive").unwrap();
    assert_eq!(drive["managedOAuth"], true);
    assert_eq!(drive["configured"], true);
    let member = rt.create_user("google-member@test.local").await;
    let (_, me) = rt.get_json("/api/v1/auth/me").await;
    sqlx::query("INSERT INTO rootcx_system.integration_connections (id, integration_id, user_id, kind) VALUES ('other-google', 'google_drive', $1, 'direct')")
        .bind(me["id"].as_str().unwrap()).execute(rt.pool()).await.unwrap();
    let (code, _) = rt.request_as(Method::POST, "/api/v1/integrations/google_drive/connections/other-google/verify", &member, Some(&json!({}))).await;
    assert_eq!(code, StatusCode::FORBIDDEN);
    let (code, start) = rt.request_as(Method::POST, "/api/v1/integrations/google_drive/auth/start", &member, Some(&json!({}))).await;
    assert_eq!(code, StatusCode::OK, "{start}");
    let url = reqwest::Url::parse(start["url"].as_str().unwrap()).unwrap();
    let params: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(params["code_challenge_method"], "S256");
    assert_eq!(params["access_type"], "offline");
    assert!(!start.to_string().contains("fixture-google-secret"));
    assert!(!integrations.to_string().contains("fixture-google-secret"));
    let state = &params["state"];
    let (code, _) = rt.get_json(&format!("/api/v1/integrations/auth/callback?error=access_denied&state={state}")).await;
    assert_eq!(code, StatusCode::OK);
    let attempt = start["attemptId"].as_str().unwrap();
    let (code, receipt) = rt.request_as(Method::GET, &format!("/api/v1/integrations/google_drive/auth/attempts/{attempt}"), &member, None).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(receipt["status"], "cancelled");
    assert!(receipt["connectionId"].is_null());
    let (code, _) = rt.get_json(&format!("/api/v1/integrations/auth/callback?error=access_denied&state={state}")).await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    rt.shutdown().await;
}

#[tokio::test]
async fn disconnect_is_atomic_when_the_connection_delete_fails() {
    let rt = TestRuntime::boot().await;
    setup_integration(&rt).await;
    let id = create_connection(&rt, &rt.token, "test_integ", "disconnect-test").await;
    sqlx::query("CREATE FUNCTION rootcx_system.reject_disconnect() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'fixture storage failure'; END $$")
        .execute(rt.pool()).await.unwrap();
    sqlx::query("CREATE TRIGGER reject_disconnect BEFORE DELETE ON rootcx_system.integration_connections FOR EACH ROW EXECUTE FUNCTION rootcx_system.reject_disconnect()")
        .execute(rt.pool()).await.unwrap();
    let path = format!("/api/v1/integrations/test_integ/connections/{id}");
    let (status, _) = rt.delete_json(&path).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    let key = format!("_conn.{id}");
    let remains: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM rootcx_system.secrets WHERE app_id='test_integ' AND key_name=$1)")
        .bind(&key).fetch_one(rt.pool()).await.unwrap();
    assert!(remains, "a failed disconnect must roll back the secret deletion too");
    sqlx::query("DROP TRIGGER reject_disconnect ON rootcx_system.integration_connections").execute(rt.pool()).await.unwrap();
    let (status, _) = rt.delete_json(&path).await;
    assert_eq!(status, StatusCode::OK);
    let remains: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM rootcx_system.secrets WHERE app_id='test_integ' AND key_name=$1)")
        .bind(&key).fetch_one(rt.pool()).await.unwrap();
    assert!(!remains, "a successful disconnect must erase the stored credential");
    rt.shutdown().await;
}

#[tokio::test]
async fn personal_bindings_are_visible_only_to_their_owner() {
    let rt = TestRuntime::boot().await;
    setup_integration(&rt).await;
    rt.install("binding_privacy", "items").await;
    let other = rt.create_user("private-binding@test.local").await;
    let a = create_connection(&rt, &rt.token, "test_integ", "a").await;
    let b = create_connection(&rt, &other, "test_integ", "b").await;
    let path = "/api/v1/apps/binding_privacy/integrations";
    for (token, conn) in [(&rt.token, &a), (&other, &b)] {
        let (status, body) = rt.request_as(Method::POST, path, token, Some(&json!({"integrationId":"test_integ","connectionId":conn,"scope":"user"}))).await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    for (token, conn) in [(&rt.token, &a), (&other, &b)] {
        let (status, rows) = rt.request_as(Method::GET, path, token, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(rows.as_array().unwrap().len(), 1, "must not expose another user's account selection");
        assert_eq!(rows[0]["connectionId"], *conn);
    }
    rt.shutdown().await;
}

#[tokio::test]
async fn simultaneous_reconnections_do_not_duplicate_an_account() {
    let rt = TestRuntime::boot().await;
    setup_integration(&rt).await;
    let (first, second, third) = tokio::join!(
        create_connection(&rt, &rt.token, "test_integ", "same-google-account"),
        create_connection(&rt, &rt.token, "test_integ", "same-google-account"),
        create_connection(&rt, &rt.token, "test_integ", "same-google-account"),
    );
    assert_eq!(first, second, "concurrent callbacks must reuse the same account");
    assert_eq!(first, third);
    let (_, rows) = rt.get_json("/api/v1/integrations/test_integ/connections").await;
    assert_eq!(rows.as_array().unwrap().len(), 1);
    rt.shutdown().await;
}
