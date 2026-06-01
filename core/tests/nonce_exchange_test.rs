mod harness;

use base64::Engine;
use harness::TestRuntime;
use reqwest::StatusCode;
use rootcx_core::auth::secure_tokens;
use serde_json::json;

async fn seed_nonce(rt: &TestRuntime, raw_nonce: &str, created_at: Option<&str>) -> uuid::Uuid {
    let user_id: (uuid::Uuid,) = sqlx::query_as(
        "SELECT id FROM rootcx_system.users WHERE email = 'admin@test.local'",
    )
    .fetch_one(rt.pool())
    .await
    .unwrap();

    let session_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO rootcx_system.sessions (id, user_id, expires_at)
         VALUES ($1, $2, now() + interval '1 hour')",
    )
    .bind(session_id)
    .bind(user_id.0)
    .execute(rt.pool())
    .await
    .unwrap();

    let nonce_hash = secure_tokens::hash(raw_nonce);
    let sql = format!(
        "INSERT INTO rootcx_system.auth_nonces (nonce_hash, user_id, session_id, created_at)
         VALUES ($1, $2, $3, {})",
        created_at.unwrap_or("now()")
    );
    sqlx::query(&sql)
        .bind(nonce_hash.as_slice())
        .bind(user_id.0)
        .bind(session_id)
        .execute(rt.pool())
        .await
        .unwrap();

    user_id.0
}

#[tokio::test]
async fn exchange_is_single_use() {
    let rt = TestRuntime::boot().await;
    let nonce = "test-nonce-single-use-aaaaaaaaaaaaaaaaaa";
    seed_nonce(&rt, nonce, None).await;

    let (s, _) = rt.post_unauthed("/api/v1/auth/nonce-exchange", &json!({"nonce": nonce})).await;
    assert_eq!(s, StatusCode::OK);

    let (s, _) = rt.post_unauthed("/api/v1/auth/nonce-exchange", &json!({"nonce": nonce})).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "second exchange must fail (single-use)");

    rt.shutdown().await;
}

#[tokio::test]
async fn exchange_rejects_expired_nonce() {
    let rt = TestRuntime::boot().await;
    let nonce = "test-nonce-expired-bbbbbbbbbbbbbbbbbbbbb";
    seed_nonce(&rt, nonce, Some("now() - interval '60 seconds'")).await;

    let (s, _) = rt.post_unauthed("/api/v1/auth/nonce-exchange", &json!({"nonce": nonce})).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "expired nonce must be rejected");

    rt.shutdown().await;
}

#[tokio::test]
async fn exchange_rejects_unknown_nonce() {
    let rt = TestRuntime::boot().await;

    let (s, _) = rt.post_unauthed(
        "/api/v1/auth/nonce-exchange",
        &json!({"nonce": "completely-random-garbage-value-xxxxxxxxx"}),
    ).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "forged nonce must be rejected");

    rt.shutdown().await;
}

#[tokio::test]
async fn nonce_is_stored_hashed_not_raw() {
    let rt = TestRuntime::boot().await;
    let nonce = "test-nonce-not-plaintext-ddddddddddddddd";
    seed_nonce(&rt, nonce, None).await;

    // Positive: the correct hash IS stored
    let expected_hash = secure_tokens::hash(nonce);
    let found: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM rootcx_system.auth_nonces WHERE nonce_hash = $1",
    )
    .bind(expected_hash.as_slice())
    .fetch_one(rt.pool())
    .await
    .unwrap();
    assert_eq!(found.0, 1, "hashed nonce must be stored");

    // Negative: raw bytes are NOT stored
    let raw_match: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM rootcx_system.auth_nonces WHERE nonce_hash = $1",
    )
    .bind(nonce.as_bytes())
    .fetch_one(rt.pool())
    .await
    .unwrap();
    assert_eq!(raw_match.0, 0, "raw nonce bytes must not match any stored hash");

    rt.shutdown().await;
}

#[tokio::test]
async fn exchange_returns_jwt_with_correct_sub() {
    let rt = TestRuntime::boot().await;
    let nonce = "test-nonce-jwt-check-eeeeeeeeeeeeeeeeeeee";
    let user_id = seed_nonce(&rt, nonce, None).await;

    let (s, body) = rt.post_unauthed("/api/v1/auth/nonce-exchange", &json!({"nonce": nonce})).await;
    assert_eq!(s, StatusCode::OK);

    let access_token = body["accessToken"].as_str().unwrap();
    let payload_b64 = access_token.split('.').nth(1).unwrap();
    let payload_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_b64)
        .unwrap();
    let claims: serde_json::Value = serde_json::from_slice(&payload_bytes).unwrap();
    assert_eq!(claims["sub"].as_str().unwrap(), user_id.to_string(),
        "JWT sub must match the user who initiated the flow");

    rt.shutdown().await;
}

#[tokio::test]
async fn legacy_delivery_puts_tokens_in_url_no_nonce() {
    let rt = TestRuntime::boot().await;

    let email = format!("legacy_{}@test.local", uuid::Uuid::new_v4());
    let (s, body) = rt.post_json(
        "/api/v1/auth/magic-link/generate",
        &json!({
            "email": email,
            "roles": [],
            "redirectUri": rt.url("/apps/test/"),
            "tokenDelivery": "query",
        }),
    ).await;
    assert_eq!(s, StatusCode::CREATED, "generate failed: {body}");

    let raw_url = body["magicLinkUrl"].as_str().unwrap();
    let token = url::Url::parse(raw_url)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "token")
        .unwrap()
        .1
        .into_owned();

    let no_redirect = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let res = no_redirect
        .get(rt.url(&format!("/api/v1/auth/magic-link/consume?token={token}")))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::TEMPORARY_REDIRECT);

    let location = res.headers().get("location").unwrap().to_str().unwrap();
    let loc = url::Url::parse(location).unwrap();

    assert!(loc.query_pairs().any(|(k, _)| k == "access_token"), "must have access_token in query");
    assert!(loc.query_pairs().any(|(k, _)| k == "refresh_token"), "must have refresh_token in query");
    assert!(loc.fragment().is_some(), "legacy must set fragment");
    assert!(!loc.query_pairs().any(|(k, _)| k == "auth_nonce"), "legacy must not emit auth_nonce");

    rt.shutdown().await;
}
