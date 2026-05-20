mod harness;
use harness::TestRuntime;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

async fn setup_integration(rt: &TestRuntime) {
    rt.install_manifest(&json!({
        "appId": "test-integ",
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
    let conn1 = create_connection(&rt, &rt.token, "test-integ", "Account A").await;
    let conn2 = create_connection(&rt, &rt.token, "test-integ", "Account B").await;
    assert_ne!(conn1, conn2);

    // List connections
    let (s, body) = rt.get_json("/api/v1/integrations/test-integ/connections").await;
    assert_eq!(s, StatusCode::OK);
    let connections = body.as_array().unwrap();
    assert!(connections.len() >= 2, "expected at least 2 connections, got {}", connections.len());

    let labels: Vec<&str> = connections.iter().filter_map(|c| c["label"].as_str()).collect();
    assert!(labels.contains(&"Account A"), "missing Account A in {labels:?}");
    assert!(labels.contains(&"Account B"), "missing Account B in {labels:?}");

    // Update label
    let (s, _) = rt.patch_json(
        &format!("/api/v1/integrations/test-integ/connections/{conn1}"),
        &json!({"label": "Primary"}),
    ).await;
    assert_eq!(s, StatusCode::OK);

    // Verify update
    let (_, body) = rt.get_json("/api/v1/integrations/test-integ/connections").await;
    let updated = body.as_array().unwrap().iter().find(|c| c["id"].as_str() == Some(&conn1)).unwrap();
    assert_eq!(updated["label"].as_str().unwrap(), "Primary");

    // Delete connection
    let (s, _) = rt.delete_json(&format!("/api/v1/integrations/test-integ/connections/{conn1}")).await;
    assert_eq!(s, StatusCode::OK);

    // Verify deletion
    let (_, body) = rt.get_json("/api/v1/integrations/test-integ/connections").await;
    let ids: Vec<&str> = body.as_array().unwrap().iter().filter_map(|c| c["id"].as_str()).collect();
    assert!(!ids.contains(&conn1.as_str()), "conn1 should be deleted");

    rt.shutdown().await;
}

#[tokio::test]
async fn connection_ownership_enforced() {
    let rt = TestRuntime::boot().await;
    setup_integration(&rt).await;

    // User A creates a connection
    let conn_id = create_connection(&rt, &rt.token, "test-integ", "User A account").await;

    // User B tries to delete it
    let user_b_token = rt.register_and_login("userb@test.local").await;
    let (s, _) = rt.request_as(
        Method::DELETE,
        &format!("/api/v1/integrations/test-integ/connections/{conn_id}"),
        &user_b_token,
        None,
    ).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "user B should not delete user A's connection");

    // User B tries to update it
    let (s, _) = rt.request_as(
        Method::PATCH,
        &format!("/api/v1/integrations/test-integ/connections/{conn_id}"),
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

    let conn1 = create_connection(&rt, &rt.token, "test-integ", "same@example.com").await;
    let conn2 = create_connection(&rt, &rt.token, "test-integ", "same@example.com").await;
    assert_eq!(conn1, conn2, "same label should return same connection_id");

    let conn3 = create_connection(&rt, &rt.token, "test-integ", "different@example.com").await;
    assert_ne!(conn1, conn3, "different label should create new connection");

    let (_, body) = rt.get_json("/api/v1/integrations/test-integ/connections").await;
    let connections = body.as_array().unwrap();
    assert_eq!(connections.len(), 2, "should have exactly 2 connections, not 3");

    rt.shutdown().await;
}

#[tokio::test]
async fn app_binding_with_connection_selection() {
    let rt = TestRuntime::boot().await;
    setup_integration(&rt).await;

    // Install a consumer app
    rt.install("consumer-app", "contacts").await;

    // Create two connections
    let conn1 = create_connection(&rt, &rt.token, "test-integ", "work@example.com").await;
    let conn2 = create_connection(&rt, &rt.token, "test-integ", "personal@example.com").await;

    // Bind app to integration with specific connection
    let (s, _) = rt.post_json(
        "/api/v1/apps/consumer-app/integrations",
        &json!({"integrationId": "test-integ", "connectionId": conn2}),
    ).await;
    assert_eq!(s, StatusCode::OK);

    // List bindings - verify connection is set
    let (s, body) = rt.get_json("/api/v1/apps/consumer-app/integrations").await;
    assert_eq!(s, StatusCode::OK);
    let bindings = body.as_array().unwrap();
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0]["integrationId"].as_str().unwrap(), "test-integ");
    assert_eq!(bindings[0]["connectionId"].as_str().unwrap(), &conn2);

    // Switch to different connection
    let (s, _) = rt.post_json(
        "/api/v1/apps/consumer-app/integrations",
        &json!({"integrationId": "test-integ", "connectionId": conn1}),
    ).await;
    assert_eq!(s, StatusCode::OK);

    let (_, body) = rt.get_json("/api/v1/apps/consumer-app/integrations").await;
    assert_eq!(body.as_array().unwrap()[0]["connectionId"].as_str().unwrap(), &conn1);

    // Unbind
    let s = rt.delete("/api/v1/apps/consumer-app/integrations/test-integ").await;
    assert_eq!(s, StatusCode::OK);

    let (_, body) = rt.get_json("/api/v1/apps/consumer-app/integrations").await;
    assert!(body.as_array().unwrap().is_empty());

    rt.shutdown().await;
}

#[tokio::test]
async fn bind_rejects_connection_not_owned_by_caller() {
    let rt = TestRuntime::boot().await;
    setup_integration(&rt).await;
    rt.install("my-app", "items").await;

    // User A creates a connection
    let conn_a = create_connection(&rt, &rt.token, "test-integ", "admin account").await;

    // User B tries to bind their app using User A's connection
    let user_b_token = rt.register_and_login("userb@test.local").await;
    let (s, body) = rt.request_as(
        Method::POST,
        "/api/v1/apps/my-app/integrations",
        &user_b_token,
        Some(&json!({"integrationId": "test-integ", "connectionId": conn_a})),
    ).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "should reject unowned connection: {body}");

    rt.shutdown().await;
}
