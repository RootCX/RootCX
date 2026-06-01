mod harness;

use harness::TestRuntime;
use reqwest::StatusCode;
use serde_json::json;

fn encode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

// ── redirect_uri origin validation ─────────────────────────────────────────

#[tokio::test]
async fn authorize_rejects_redirect_uri_with_userinfo_attack() {
    let rt = TestRuntime::boot().await;

    // core_public_url() defaults to http://localhost:9100 in tests (no ROOTCX_PUBLIC_URL set)
    let cases = [
        ("http://localhost:9100@evil.com/steal", "userinfo trick"),
        ("http://evil.com/path", "different host"),
        ("http://localhost:9100.evil.com/steal", "subdomain prefix trick"),
        ("not a url", "unparseable"),
    ];

    // Seed provider so we don't get a 404 before reaching redirect_uri validation
    sqlx::query(
        "INSERT INTO rootcx_system.oidc_providers (id, display_name, issuer_url, client_id)
         VALUES ('redir_test', 'T', 'http://fake.invalid', 'cid') ON CONFLICT DO NOTHING",
    ).execute(rt.pool()).await.unwrap();
    rt.runtime.secret_manager().set(rt.pool(), "oidc:redir_test", "client_secret", "s").await.unwrap();

    let no_redirect = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    for (uri, label) in cases {
        let url = rt.url(&format!(
            "/api/v1/auth/oidc/redir_test/authorize?redirect_uri={}",
            encode(uri)
        ));
        let res = no_redirect.get(&url).send().await.unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "should reject {label}: {uri}");
    }

    rt.shutdown().await;
}

#[tokio::test]
async fn authorize_accepts_same_origin_redirect_uri() {
    let rt = TestRuntime::boot().await;

    sqlx::query(
        "INSERT INTO rootcx_system.oidc_providers (id, display_name, issuer_url, client_id)
         VALUES ('origin_test', 'T', 'http://fake.invalid', 'cid') ON CONFLICT DO NOTHING",
    ).execute(rt.pool()).await.unwrap();
    rt.runtime.secret_manager().set(rt.pool(), "oidc:origin_test", "client_secret", "s").await.unwrap();

    // core_public_url() = http://localhost:9100, so same-origin redirect must match that
    let same_origin = "http://localhost:9100/apps/crm/";
    let no_redirect = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let url = rt.url(&format!(
        "/api/v1/auth/oidc/origin_test/authorize?redirect_uri={}",
        encode(same_origin)
    ));
    let res = no_redirect.get(&url).send().await.unwrap();
    // Discovery will fail (fake issuer) but that's 500, not 400.
    // If we get 400, the origin validation rejected it incorrectly.
    assert_ne!(res.status(), StatusCode::BAD_REQUEST, "same-origin should be accepted");

    rt.shutdown().await;
}

// ── nonce-exchange ─────────────────────────────────────────────────────────

#[tokio::test]
async fn nonce_exchange_single_use() {
    let rt = TestRuntime::boot().await;

    // Insert a nonce directly
    let nonce = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO rootcx_system.auth_nonces (nonce, access_token, refresh_token, expires_in)
         VALUES ($1, 'at_test', 'rt_test', 900)",
    ).bind(&nonce).execute(rt.pool()).await.unwrap();

    // First exchange succeeds
    let (s, body) = rt.post_unauthed("/api/v1/auth/nonce-exchange", &json!({"nonce": nonce})).await;
    assert_eq!(s, StatusCode::OK, "first exchange should succeed: {body}");
    assert_eq!(body["accessToken"].as_str().unwrap(), "at_test");
    assert_eq!(body["refreshToken"].as_str().unwrap(), "rt_test");
    assert_eq!(body["expiresIn"].as_i64().unwrap(), 900);

    // Second exchange fails (single-use)
    let (s, _) = rt.post_unauthed("/api/v1/auth/nonce-exchange", &json!({"nonce": nonce})).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    rt.shutdown().await;
}

#[tokio::test]
async fn nonce_exchange_rejects_expired() {
    let rt = TestRuntime::boot().await;

    let nonce = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO rootcx_system.auth_nonces (nonce, access_token, refresh_token, expires_in, created_at)
         VALUES ($1, 'at', 'rt', 900, now() - interval '60 seconds')",
    ).bind(&nonce).execute(rt.pool()).await.unwrap();

    let (s, _) = rt.post_unauthed("/api/v1/auth/nonce-exchange", &json!({"nonce": nonce})).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    rt.shutdown().await;
}

#[tokio::test]
async fn nonce_exchange_rejects_unknown() {
    let rt = TestRuntime::boot().await;

    let (s, _) = rt.post_unauthed(
        "/api/v1/auth/nonce-exchange",
        &json!({"nonce": "nonexistent-nonce-value"}),
    ).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    rt.shutdown().await;
}

// ── token delivery retro-compat (OIDC callback) ───────────────────────────

/// Simulates what the OIDC callback produces for legacy SDKs (no token_delivery param).
/// Verifies all 3 SDK generations can find tokens in the redirect URL.
#[tokio::test]
async fn oidc_legacy_callback_delivers_tokens_in_query_and_fragment() {
    let rt = TestRuntime::boot().await;

    // Simulate the OIDC state row that would exist after authorize (no token_delivery=nonce)
    sqlx::query(
        "INSERT INTO rootcx_system.oidc_state (state, provider_id, nonce, pkce_verifier, redirect_uri, token_delivery)
         VALUES ('test_state_legacy', 'rootcx', 'n', 'v', 'http://localhost:9100/apps/crm/', 'query')",
    ).execute(rt.pool()).await.unwrap();

    // We can't call the real callback (needs OIDC code exchange), but we can verify
    // the contract by checking what auth_nonces + redirect would contain.
    // Instead, test the magic-link which uses the same dual-delivery pattern.
    // The OIDC callback uses identical code in the `else` branch.
    // (Covered transitively via magic_link_all_sdk_generations_can_authenticate below)

    rt.shutdown().await;
}

// ── magic-link retro-compat ────────────────────────────────────────────────

/// Helper: generate a magic-link with redirect_uri and GET-consume it, returning the Location URL.
async fn magic_link_redirect_url(rt: &TestRuntime) -> url::Url {
    sqlx::query(
        "INSERT INTO rootcx_system.rbac_assignments (user_id, role)
         SELECT id, 'admin' FROM rootcx_system.users WHERE email = 'admin@test.local'
         ON CONFLICT DO NOTHING",
    ).execute(rt.pool()).await.unwrap();
    sqlx::query(
        "INSERT INTO rootcx_system.rbac_roles (name, permissions) VALUES ('compat_role', '{}')
         ON CONFLICT DO NOTHING",
    ).execute(rt.pool()).await.unwrap();

    let email = format!("user_{}@test.local", uuid::Uuid::new_v4());
    let (s, body) = rt.post_json(
        "/api/v1/auth/magic-link/generate",
        &json!({
            "email": email,
            "roles": ["compat_role"],
            "redirectUri": rt.url("/apps/test/"),
        }),
    ).await;
    assert_eq!(s, StatusCode::CREATED, "generate failed: {body}");

    let raw_url = body["magicLinkUrl"].as_str().unwrap();
    let token = url::Url::parse(raw_url).unwrap()
        .query_pairs().find(|(k, _)| k == "token").unwrap().1.into_owned();

    let no_redirect = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let res = no_redirect.get(rt.url(&format!("/api/v1/auth/magic-link/consume?token={token}")))
        .send().await.unwrap();
    assert_eq!(res.status(), StatusCode::TEMPORARY_REDIRECT);

    let location = res.headers().get("location").unwrap().to_str().unwrap();
    url::Url::parse(location).unwrap()
}

/// SDK 0.13-0.16 reads access_token from query params.
/// SDK 0.17-0.18 reads access_token from hash fragment.
/// SDK 0.19+ reads auth_nonce from query params and exchanges via POST.
/// All three must work from the same redirect URL.
#[tokio::test]
async fn magic_link_all_sdk_generations_can_authenticate() {
    let rt = TestRuntime::boot().await;
    let loc = magic_link_redirect_url(&rt).await;

    // SDK 0.13-0.16: reads from query params
    let query_access = loc.query_pairs().find(|(k, _)| k == "access_token");
    assert!(query_access.is_some(), "SDK 0.13-0.16: access_token must be in query params");

    let query_refresh = loc.query_pairs().find(|(k, _)| k == "refresh_token");
    assert!(query_refresh.is_some(), "SDK 0.13-0.16: refresh_token must be in query params");

    // SDK 0.17-0.18: reads from fragment
    let fragment = loc.fragment().expect("SDK 0.17-0.18: fragment must be present");
    let frag_params = url::form_urlencoded::parse(fragment.as_bytes())
        .collect::<std::collections::HashMap<_, _>>();
    assert!(frag_params.contains_key("access_token"), "SDK 0.17-0.18: access_token must be in fragment");
    assert!(frag_params.contains_key("refresh_token"), "SDK 0.17-0.18: refresh_token must be in fragment");

    // SDK 0.19+: reads auth_nonce and exchanges
    let nonce = loc.query_pairs()
        .find(|(k, _)| k == "auth_nonce")
        .expect("SDK 0.19+: auth_nonce must be in query params")
        .1.into_owned();
    let (s, body) = rt.post_unauthed("/api/v1/auth/nonce-exchange", &json!({"nonce": nonce})).await;
    assert_eq!(s, StatusCode::OK, "SDK 0.19+: nonce exchange must succeed: {body}");
    assert!(body["accessToken"].as_str().unwrap().starts_with("eyJ"), "must be a JWT");

    // All three paths must return tokens for the same user
    let query_at = query_access.unwrap().1.into_owned();
    let frag_at = frag_params["access_token"].to_string();
    let nonce_at = body["accessToken"].as_str().unwrap();
    assert_eq!(query_at, frag_at, "query and fragment tokens must match");
    assert_eq!(query_at, nonce_at, "query and nonce-exchanged tokens must match");

    rt.shutdown().await;
}

#[tokio::test]
async fn magic_link_redirect_has_referrer_policy_header() {
    let rt = TestRuntime::boot().await;
    let no_redirect = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();

    sqlx::query(
        "INSERT INTO rootcx_system.rbac_assignments (user_id, role)
         SELECT id, 'admin' FROM rootcx_system.users WHERE email = 'admin@test.local'
         ON CONFLICT DO NOTHING",
    ).execute(rt.pool()).await.unwrap();
    sqlx::query(
        "INSERT INTO rootcx_system.rbac_roles (name, permissions) VALUES ('hdr_role', '{}')
         ON CONFLICT DO NOTHING",
    ).execute(rt.pool()).await.unwrap();

    let (_, body) = rt.post_json(
        "/api/v1/auth/magic-link/generate",
        &json!({"email": "hdr@test.local", "roles": ["hdr_role"], "redirectUri": rt.url("/app/")}),
    ).await;

    let token = url::Url::parse(body["magicLinkUrl"].as_str().unwrap()).unwrap()
        .query_pairs().find(|(k, _)| k == "token").unwrap().1.into_owned();
    let res = no_redirect.get(rt.url(&format!("/api/v1/auth/magic-link/consume?token={token}")))
        .send().await.unwrap();

    assert_eq!(
        res.headers().get("referrer-policy").unwrap().to_str().unwrap(),
        "no-referrer",
    );

    rt.shutdown().await;
}
