mod harness;
use harness::TestRuntime;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

/// Install a minimal app with the `public.share` permission and a public RPC declaration.
async fn install_sharing_app(rt: &TestRuntime) {
    let manifest = json!({
        "appId": "sharetest",
        "name": "Share Test",
        "version": "1.0.0",
        "dataContract": [{
            "entityName": "board",
            "fields": [
                { "name": "title", "type": "text", "required": true }
            ]
        }],
        "permissions": {
            "permissions": [
                { "key": "board.read", "description": "read boards" },
                { "key": "public.share", "description": "create public share links" }
            ]
        },
        "public": {
            "rpcs": [
                { "name": "get_public_board", "scope": ["board_id"] },
                { "name": "list_public", "scope": [] }
            ]
        }
    });
    rt.install_manifest(&manifest).await;
}

#[tokio::test]
async fn create_share_and_resolve_via_info_endpoint() {
    let rt = TestRuntime::boot().await;
    install_sharing_app(&rt).await;

    // Create a board record
    let board = rt.create("sharetest", "board", &json!({"title": "My Board"})).await;
    let board_id = board["id"].as_str().unwrap();

    // Create a share
    let (s, body) = rt.post_json(
        "/api/v1/apps/sharetest/public-shares",
        &json!({"context": {"board_id": board_id}}),
    ).await;
    assert_eq!(s, StatusCode::CREATED, "create share failed: {body}");
    let token = body["token"].as_str().unwrap();
    assert_eq!(token.len(), 43, "token should be 43 chars base64url");
    assert!(!token.is_empty());
    assert_eq!(body["context"]["board_id"].as_str().unwrap(), board_id);

    // Resolve the token via GET /api/v1/public/share/info (with share token as Bearer)
    let res = rt.client
        .get(rt.url("/api/v1/public/share/info"))
        .bearer_auth(token)
        .send().await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let info: Value = res.json().await.unwrap();
    assert_eq!(info["appId"].as_str().unwrap(), "sharetest");
    assert_eq!(info["context"]["board_id"].as_str().unwrap(), board_id);

    rt.shutdown().await;
}

#[tokio::test]
async fn revoked_share_returns_401() {
    let rt = TestRuntime::boot().await;
    install_sharing_app(&rt).await;

    let board = rt.create("sharetest", "board", &json!({"title": "Board"})).await;
    let board_id = board["id"].as_str().unwrap();

    // Create share
    let (_, body) = rt.post_json(
        "/api/v1/apps/sharetest/public-shares",
        &json!({"context": {"board_id": board_id}}),
    ).await;
    let token = body["token"].as_str().unwrap().to_string();
    let share_id = body["id"].as_str().unwrap();

    // Verify it works
    let res = rt.client.get(rt.url("/api/v1/public/share/info"))
        .bearer_auth(&token).send().await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Revoke
    let s = rt.delete(&format!("/api/v1/apps/sharetest/public-shares/{share_id}")).await;
    assert_eq!(s, StatusCode::OK);

    // Now the token should fail
    let res = rt.client.get(rt.url("/api/v1/public/share/info"))
        .bearer_auth(&token).send().await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    rt.shutdown().await;
}

#[tokio::test]
async fn share_info_rejects_jwt_bearer() {
    let rt = TestRuntime::boot().await;

    // A valid JWT should NOT resolve as a share
    let res = rt.client.get(rt.url("/api/v1/public/share/info"))
        .bearer_auth(&rt.token)
        .send().await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    rt.shutdown().await;
}

#[tokio::test]
async fn share_info_rejects_invalid_token() {
    let rt = TestRuntime::boot().await;

    // Random 43-char string that isn't a valid share token
    let fake = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopq";
    let res = rt.client.get(rt.url("/api/v1/public/share/info"))
        .bearer_auth(fake).send().await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    rt.shutdown().await;
}

#[tokio::test]
async fn share_info_rejects_no_bearer() {
    let rt = TestRuntime::boot().await;

    let res = rt.client.get(rt.url("/api/v1/public/share/info"))
        .send().await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    rt.shutdown().await;
}

#[tokio::test]
async fn create_share_requires_auth() {
    let rt = TestRuntime::boot().await;
    install_sharing_app(&rt).await;

    let (s, _) = rt.post_unauthed(
        "/api/v1/apps/sharetest/public-shares",
        &json!({"context": {"board_id": "x"}}),
    ).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    rt.shutdown().await;
}

#[tokio::test]
async fn create_share_is_idempotent() {
    let rt = TestRuntime::boot().await;
    install_sharing_app(&rt).await;

    let board = rt.create("sharetest", "board", &json!({"title": "Board"})).await;
    let board_id = board["id"].as_str().unwrap();
    let ctx = json!({"context": {"board_id": board_id}});

    // First create → 201, token returned
    let (s1, b1) = rt.post_json("/api/v1/apps/sharetest/public-shares", &ctx).await;
    assert_eq!(s1, StatusCode::CREATED);
    assert!(!b1["token"].as_str().unwrap().is_empty());

    // Second create with same context → 200, token empty (idempotent, can't recover original)
    let (s2, b2) = rt.post_json("/api/v1/apps/sharetest/public-shares", &ctx).await;
    assert_eq!(s2, StatusCode::OK);
    assert!(b2["token"].as_str().unwrap().is_empty());
    assert_eq!(b1["id"], b2["id"]);

    rt.shutdown().await;
}

#[tokio::test]
async fn revoke_only_works_for_creator() {
    let rt = TestRuntime::boot().await;
    install_sharing_app(&rt).await;

    let board = rt.create("sharetest", "board", &json!({"title": "Board"})).await;
    let board_id = board["id"].as_str().unwrap();

    let (_, body) = rt.post_json(
        "/api/v1/apps/sharetest/public-shares",
        &json!({"context": {"board_id": board_id}}),
    ).await;
    let share_id = body["id"].as_str().unwrap();

    // Register another user
    let other_token = rt.register_and_login("other@test.local").await;

    // Other user tries to revoke → 404 (not 403, to avoid leaking share existence)
    let (s, _) = rt.request_as(
        Method::DELETE,
        &format!("/api/v1/apps/sharetest/public-shares/{share_id}"),
        &other_token,
        None,
    ).await;
    // Should get 403 (no permission) or 404 (not their share)
    assert!(s == StatusCode::FORBIDDEN || s == StatusCode::NOT_FOUND,
        "expected 403/404 for non-creator revoke, got {s}");

    rt.shutdown().await;
}

#[tokio::test]
async fn rpc_with_share_token_scope_mismatch_returns_403() {
    let rt = TestRuntime::boot().await;
    install_sharing_app(&rt).await;

    let board = rt.create("sharetest", "board", &json!({"title": "Board A"})).await;
    let board_a_id = board["id"].as_str().unwrap().to_string();
    let board_b = rt.create("sharetest", "board", &json!({"title": "Board B"})).await;
    let board_b_id = board_b["id"].as_str().unwrap();

    // Create share scoped to board A
    let (_, body) = rt.post_json(
        "/api/v1/apps/sharetest/public-shares",
        &json!({"context": {"board_id": board_a_id}}),
    ).await;
    let token = body["token"].as_str().unwrap().to_string();

    // Try to call RPC with board B's id → scope mismatch → 403
    let res = rt.client
        .post(rt.url("/api/v1/apps/sharetest/rpc"))
        .bearer_auth(&token)
        .json(&json!({"method": "get_public_board", "params": {"board_id": board_b_id}}))
        .send().await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);

    rt.shutdown().await;
}

#[tokio::test]
async fn rpc_non_public_method_rejected_with_share_token() {
    let rt = TestRuntime::boot().await;
    install_sharing_app(&rt).await;

    let board = rt.create("sharetest", "board", &json!({"title": "Board"})).await;
    let board_id = board["id"].as_str().unwrap();

    let (_, body) = rt.post_json(
        "/api/v1/apps/sharetest/public-shares",
        &json!({"context": {"board_id": board_id}}),
    ).await;
    let token = body["token"].as_str().unwrap().to_string();

    // Call a method not listed in manifest.public.rpcs → 403
    let res = rt.client
        .post(rt.url("/api/v1/apps/sharetest/rpc"))
        .bearer_auth(&token)
        .json(&json!({"method": "delete_everything", "params": {}}))
        .send().await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);

    rt.shutdown().await;
}

#[tokio::test]
async fn rpc_anonymous_on_non_public_method_returns_401() {
    let rt = TestRuntime::boot().await;
    install_sharing_app(&rt).await;

    // Call RPC with no auth at all → 401
    let (s, _) = rt.post_unauthed(
        "/api/v1/apps/sharetest/rpc",
        &json!({"method": "delete_everything", "params": {}}),
    ).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    rt.shutdown().await;
}

#[tokio::test]
async fn share_token_cannot_access_other_apps_rpc() {
    let rt = TestRuntime::boot().await;
    install_sharing_app(&rt).await;

    // Install a second app
    rt.install_manifest(&json!({
        "appId": "other_app", "name": "Other", "version": "1.0.0",
        "dataContract": [{"entityName": "item", "fields": [{"name": "name", "type": "text", "required": true}]}],
        "public": { "rpcs": [{"name": "list_items", "scope": []}] }
    })).await;

    // Create share for sharetest
    let board = rt.create("sharetest", "board", &json!({"title": "Board"})).await;
    let (_, body) = rt.post_json(
        "/api/v1/apps/sharetest/public-shares",
        &json!({"context": {"board_id": board["id"].as_str().unwrap()}}),
    ).await;
    let token = body["token"].as_str().unwrap().to_string();

    // Use sharetest's token to call other_app's public RPC → 403 (cross-app)
    let res = rt.client
        .post(rt.url("/api/v1/apps/other_app/rpc"))
        .bearer_auth(&token)
        .json(&json!({"method": "list_items", "params": {}}))
        .send().await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);

    rt.shutdown().await;
}
