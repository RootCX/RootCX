mod harness;

use harness::TestRuntime;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

fn extract_token(raw: &str) -> String {
    let url = url::Url::parse(raw).expect("invalid magic_link_url");
    url.query_pairs()
        .find(|(k, _)| k == "token")
        .map(|(_, v)| v.into_owned())
        .expect("token query param missing")
}

async fn create_role_with_perms(rt: &TestRuntime, name: &str, perms: &[&str]) {
    sqlx::query(
        "INSERT INTO rootcx_system.rbac_roles (name, permissions) VALUES ($1, $2) \
         ON CONFLICT (name) DO UPDATE SET permissions = EXCLUDED.permissions",
    )
    .bind(name)
    .bind(perms)
    .execute(rt.pool())
    .await
    .unwrap();
}

async fn assign_role(rt: &TestRuntime, user_email: &str, role: &str) {
    let (user_id,): (uuid::Uuid,) =
        sqlx::query_as("SELECT id FROM rootcx_system.users WHERE email = $1")
            .bind(user_email)
            .fetch_one(rt.pool())
            .await
            .unwrap();
    sqlx::query(
        "INSERT INTO rootcx_system.rbac_assignments (user_id, role) VALUES ($1, $2) \
         ON CONFLICT DO NOTHING",
    )
    .bind(user_id)
    .bind(role)
    .execute(rt.pool())
    .await
    .unwrap();
}

/// The first-user-admin conditional in /auth/register no-ops because
/// seed_assistant claims it during boot. Tests bypass via direct SQL.
async fn promote_harness_admin(rt: &TestRuntime) {
    assign_role(rt, "admin@test.local", "admin").await;
}

#[tokio::test]
async fn happy_path_generate_consume_returns_valid_jwt() {
    let rt = TestRuntime::boot().await;
    promote_harness_admin(&rt).await;
    create_role_with_perms(&rt, "volunteer", &["app:demo:read"]).await;

    let (s, body) = rt.post_json(
        "/api/v1/auth/magic-link/generate",
        &json!({"email": "bob@volunteer.org", "roles": ["volunteer"]}),
    ).await;
    assert_eq!(s, StatusCode::CREATED, "generate failed: {body}");
    let token = extract_token(body["magicLinkUrl"].as_str().unwrap());

    let (s, body) = rt.post_unauthed(
        "/api/v1/auth/magic-link/consume",
        &json!({"token": token}),
    ).await;
    assert_eq!(s, StatusCode::OK, "consume failed: {body}");
    let access = body["accessToken"].as_str().unwrap();
    assert_eq!(body["user"]["email"].as_str().unwrap(), "bob@volunteer.org");

    // Use the JWT to hit /auth/me
    let me = rt.client.get(rt.url("/api/v1/auth/me"))
        .bearer_auth(access).send().await.unwrap();
    assert_eq!(me.status(), StatusCode::OK);
    let me_body: Value = me.json().await.unwrap();
    assert_eq!(me_body["email"].as_str().unwrap(), "bob@volunteer.org");

    // Verify the role was conferred
    let assigned: Vec<(String,)> = sqlx::query_as(
        "SELECT role FROM rootcx_system.rbac_assignments a \
         JOIN rootcx_system.users u ON u.id = a.user_id \
         WHERE u.email = $1",
    )
    .bind("bob@volunteer.org")
    .fetch_all(rt.pool())
    .await
    .unwrap();
    assert!(assigned.iter().any(|(r,)| r == "volunteer"));

    rt.shutdown().await;
}

#[tokio::test]
async fn consume_is_single_use() {
    let rt = TestRuntime::boot().await;
    promote_harness_admin(&rt).await;
    create_role_with_perms(&rt, "volunteer", &[]).await;

    let (_, body) = rt.post_json(
        "/api/v1/auth/magic-link/generate",
        &json!({"email": "alice@volunteer.org", "roles": ["volunteer"]}),
    ).await;
    let token = extract_token(body["magicLinkUrl"].as_str().unwrap());

    // First consume succeeds
    let (s, _) = rt.post_unauthed("/api/v1/auth/magic-link/consume", &json!({"token": token})).await;
    assert_eq!(s, StatusCode::OK);

    // Second consume fails
    let (s, _) = rt.post_unauthed("/api/v1/auth/magic-link/consume", &json!({"token": token})).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    rt.shutdown().await;
}

#[tokio::test]
async fn consume_rejects_expired_token() {
    let rt = TestRuntime::boot().await;
    promote_harness_admin(&rt).await;
    create_role_with_perms(&rt, "volunteer", &[]).await;

    let (_, body) = rt.post_json(
        "/api/v1/auth/magic-link/generate",
        &json!({"email": "late@volunteer.org", "roles": ["volunteer"], "expiresInSeconds": 60}),
    ).await;
    let token = extract_token(body["magicLinkUrl"].as_str().unwrap());

    // Force expiry in DB
    sqlx::query("UPDATE rootcx_system.magic_link_tokens SET expires_at = now() - interval '1 second'")
        .execute(rt.pool()).await.unwrap();

    let (s, _) = rt.post_unauthed("/api/v1/auth/magic-link/consume", &json!({"token": token})).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    rt.shutdown().await;
}

#[tokio::test]
async fn consume_rejects_unknown_token() {
    let rt = TestRuntime::boot().await;
    // Random 43-char base64url string never inserted in the DB.
    let fake = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopq";
    let (s, _) = rt.post_unauthed("/api/v1/auth/magic-link/consume", &json!({"token": fake})).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    rt.shutdown().await;
}

#[tokio::test]
async fn consume_rejects_malformed_token() {
    let rt = TestRuntime::boot().await;
    for bad in ["", "short", "way-too-long-token-that-exceeds-43-chars-by-quite-a-lot", "has.dot.in.it.exactly.43.chars.long.with.dots"] {
        let (s, _) = rt.post_unauthed("/api/v1/auth/magic-link/consume", &json!({"token": bad})).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "bad token should be 401: {bad:?}");
    }
    rt.shutdown().await;
}

#[tokio::test]
async fn consume_reuses_existing_user_by_email() {
    let rt = TestRuntime::boot().await;
    promote_harness_admin(&rt).await;
    create_role_with_perms(&rt, "volunteer", &[]).await;

    // Pre-create the user
    rt.post_unauthed("/api/v1/auth/register", &json!({"email": "existing@x.com", "password": "Str0ngPass1"})).await;
    let (existing_id,): (uuid::Uuid,) = sqlx::query_as("SELECT id FROM rootcx_system.users WHERE email = $1")
        .bind("existing@x.com").fetch_one(rt.pool()).await.unwrap();

    let (_, body) = rt.post_json(
        "/api/v1/auth/magic-link/generate",
        &json!({"email": "existing@x.com", "roles": ["volunteer"]}),
    ).await;
    let token = extract_token(body["magicLinkUrl"].as_str().unwrap());

    let (s, body) = rt.post_unauthed("/api/v1/auth/magic-link/consume", &json!({"token": token})).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["user"]["id"].as_str().unwrap(), existing_id.to_string());

    // Only one row in users with that email
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM rootcx_system.users WHERE email = $1")
        .bind("existing@x.com").fetch_one(rt.pool()).await.unwrap();
    assert_eq!(count, 1);

    rt.shutdown().await;
}

#[tokio::test]
async fn generate_normalizes_email_case_and_whitespace() {
    let rt = TestRuntime::boot().await;
    promote_harness_admin(&rt).await;
    create_role_with_perms(&rt, "volunteer", &[]).await;

    let (s, body) = rt.post_json(
        "/api/v1/auth/magic-link/generate",
        &json!({"email": "  Bob@VOLUNTEER.ORG  ", "roles": ["volunteer"]}),
    ).await;
    assert_eq!(s, StatusCode::CREATED, "{body}");

    let token = extract_token(body["magicLinkUrl"].as_str().unwrap());
    let (s, body) = rt.post_unauthed("/api/v1/auth/magic-link/consume", &json!({"token": token})).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["user"]["email"].as_str().unwrap(), "bob@volunteer.org");
    rt.shutdown().await;
}

#[tokio::test]
async fn generate_rejects_invalid_email() {
    let rt = TestRuntime::boot().await;
    promote_harness_admin(&rt).await;
    for bad in ["", "no-at-sign", "  "] {
        let (s, _) = rt.post_json(
            "/api/v1/auth/magic-link/generate",
            &json!({"email": bad, "roles": []}),
        ).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "bad email should be 400: {bad:?}");
    }
    rt.shutdown().await;
}

#[tokio::test]
async fn generate_rejects_unknown_role() {
    let rt = TestRuntime::boot().await;
    promote_harness_admin(&rt).await;
    let (s, body) = rt.post_json(
        "/api/v1/auth/magic-link/generate",
        &json!({"email": "x@y.com", "roles": ["does-not-exist"]}),
    ).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    rt.shutdown().await;
}

#[tokio::test]
async fn generate_rejects_unsafe_redirect_uri() {
    let rt = TestRuntime::boot().await;
    promote_harness_admin(&rt).await;
    for bad in [
        "javascript:alert(1)",
        "file:///etc/passwd",
        "https://attacker:pwd@victim.com/",
        "not a url",
    ] {
        let (s, _) = rt.post_json(
            "/api/v1/auth/magic-link/generate",
            &json!({"email": "x@y.com", "roles": [], "redirectUri": bad}),
        ).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "unsafe redirect should be 400: {bad}");
    }
    rt.shutdown().await;
}

#[tokio::test]
async fn generate_rejects_caller_without_auth_invite() {
    let rt = TestRuntime::boot().await;
    create_role_with_perms(&rt, "viewer", &["app:demo:read"]).await;
    create_role_with_perms(&rt, "volunteer", &[]).await;

    let token = rt.register_and_login("nobody@x.com").await;
    // Give them a non-admin role without auth.invite
    assign_role(&rt, "nobody@x.com", "viewer").await;

    let (s, body) = rt.request_as(
        Method::POST,
        "/api/v1/auth/magic-link/generate",
        &token,
        Some(&json!({"email": "victim@x.com", "roles": ["volunteer"]})),
    ).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{body}");

    rt.shutdown().await;
}

#[tokio::test]
async fn generate_prevents_role_escalation_for_non_admin() {
    let rt = TestRuntime::boot().await;
    // Inviter role: has auth.invite + holds "volunteer" but NOT "manager"
    create_role_with_perms(&rt, "inviter", &["auth.invite"]).await;
    create_role_with_perms(&rt, "volunteer", &[]).await;
    create_role_with_perms(&rt, "manager", &["app:secret:*"]).await;

    let token = rt.register_and_login("inviter@x.com").await;
    assign_role(&rt, "inviter@x.com", "inviter").await;
    assign_role(&rt, "inviter@x.com", "volunteer").await;

    // Conferring "volunteer" (owned) — OK
    let (s, _) = rt.request_as(
        Method::POST,
        "/api/v1/auth/magic-link/generate",
        &token,
        Some(&json!({"email": "newb@x.com", "roles": ["volunteer"]})),
    ).await;
    assert_eq!(s, StatusCode::CREATED, "owned role should succeed");

    // Conferring "manager" (NOT owned) — 403
    let (s, _) = rt.request_as(
        Method::POST,
        "/api/v1/auth/magic-link/generate",
        &token,
        Some(&json!({"email": "newb@x.com", "roles": ["manager"]})),
    ).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "non-owned role must be rejected");

    rt.shutdown().await;
}

#[tokio::test]
async fn consume_rejects_anonymous_when_no_token() {
    let rt = TestRuntime::boot().await;
    // Missing token field
    let r = rt.client.post(rt.url("/api/v1/auth/magic-link/consume"))
        .json(&json!({})).send().await.unwrap();
    assert!(r.status().is_client_error());
    rt.shutdown().await;
}

#[tokio::test]
async fn token_is_stored_hashed_not_plain() {
    let rt = TestRuntime::boot().await;
    promote_harness_admin(&rt).await;
    create_role_with_perms(&rt, "volunteer", &[]).await;

    let (_, body) = rt.post_json(
        "/api/v1/auth/magic-link/generate",
        &json!({"email": "secret@x.com", "roles": ["volunteer"]}),
    ).await;
    let raw = extract_token(body["magicLinkUrl"].as_str().unwrap());

    // The raw token should not appear in any stored column
    let (stored_hash,): (Vec<u8>,) = sqlx::query_as(
        "SELECT token_hash FROM rootcx_system.magic_link_tokens WHERE email = $1",
    )
    .bind("secret@x.com")
    .fetch_one(rt.pool())
    .await
    .unwrap();
    assert_eq!(stored_hash.len(), 32, "hash must be 32 bytes (SHA-256)");
    assert_ne!(stored_hash, raw.as_bytes(), "raw token must never be stored");
    rt.shutdown().await;
}

#[tokio::test]
async fn redirect_uri_is_returned_to_consumer() {
    let rt = TestRuntime::boot().await;
    promote_harness_admin(&rt).await;
    create_role_with_perms(&rt, "volunteer", &[]).await;

    let (_, body) = rt.post_json(
        "/api/v1/auth/magic-link/generate",
        &json!({
            "email": "go@x.com",
            "roles": ["volunteer"],
            "redirectUri": "https://pulsecrm.foundation/auth/callback"
        }),
    ).await;
    let token = extract_token(body["magicLinkUrl"].as_str().unwrap());

    let (s, body) = rt.post_unauthed("/api/v1/auth/magic-link/consume", &json!({"token": token})).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        body["redirectUri"].as_str().unwrap(),
        "https://pulsecrm.foundation/auth/callback",
    );
    rt.shutdown().await;
}
