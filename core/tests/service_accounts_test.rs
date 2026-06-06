//! Service-account contract tests: non-human principals, client-credentials
//! auth (RFC 6749 section 4.4), disable lifecycle, and the single act-as gate
//! with anti-escalation. Black-box over the governance model.

mod harness;

use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use uuid::Uuid;

async fn create_sa(rt: &harness::TestRuntime, slug: &str) -> (Uuid, String) {
    let (s, body) = rt
        .request_as(Method::POST, "/api/v1/service-accounts", &rt.token, Some(&json!({ "slug": slug })))
        .await;
    assert_eq!(s, StatusCode::CREATED, "create SA: {body}");
    let id: Uuid = body["id"].as_str().unwrap().parse().unwrap();
    (id, body["email"].as_str().unwrap().to_string())
}

async fn create_credential_with_id(rt: &harness::TestRuntime, sa: Uuid) -> (Uuid, String) {
    let (s, body) = rt
        .request_as(Method::POST, &format!("/api/v1/service-accounts/{sa}/credentials"), &rt.token, Some(&json!({ "name": "default" })))
        .await;
    assert_eq!(s, StatusCode::CREATED, "create credential: {body}");
    (body["id"].as_str().unwrap().parse().unwrap(), body["key"].as_str().unwrap().to_string())
}

async fn create_credential(rt: &harness::TestRuntime, sa: Uuid) -> String {
    create_credential_with_id(rt, sa).await.1
}

/// Client-credentials token exchange (RFC 6749 4.4).
async fn token_exchange(rt: &harness::TestRuntime, sa: Uuid, secret: &str) -> reqwest::Response {
    let cid = sa.to_string();
    rt.client.post(rt.url("/api/v1/auth/token"))
        .form(&[("grant_type", "client_credentials"), ("client_id", cid.as_str()), ("client_secret", secret)])
        .send().await.unwrap()
}

async fn token_status(rt: &harness::TestRuntime, sa: Uuid, secret: &str) -> StatusCode {
    token_exchange(rt, sa, secret).await.status()
}

async fn token_access(rt: &harness::TestRuntime, sa: Uuid, secret: &str) -> String {
    let body: Value = token_exchange(rt, sa, secret).await.json().await.unwrap();
    body["access_token"].as_str().unwrap().to_string()
}

/// Assign exactly `perms` to an existing principal via a fresh dedicated role.
async fn grant_perms(pool: &sqlx::PgPool, uid: Uuid, perms: &[&str]) {
    sqlx::query("DELETE FROM rootcx_system.rbac_assignments WHERE user_id = $1").bind(uid).execute(pool).await.unwrap();
    let role = format!("role_{}", uid.simple());
    let list: Vec<String> = perms.iter().map(|s| s.to_string()).collect();
    sqlx::query("INSERT INTO rootcx_system.rbac_roles (name, permissions) VALUES ($1, $2) ON CONFLICT (name) DO UPDATE SET permissions = EXCLUDED.permissions")
        .bind(&role).bind(&list).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO rootcx_system.rbac_assignments (user_id, role) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(uid).bind(&role).execute(pool).await.unwrap();
}

/// Register a human, log in, and give them exactly `perms`.
async fn human_with(rt: &harness::TestRuntime, email: &str, perms: &[&str]) -> (String, Uuid) {
    let tok = rt.register_and_login(email).await;
    let uid: Uuid = sqlx::query_scalar("SELECT id FROM rootcx_system.users WHERE email = $1")
        .bind(email).fetch_one(rt.pool()).await.unwrap();
    grant_perms(rt.pool(), uid, perms).await;
    (tok, uid)
}

async fn grant_act_as(rt: &harness::TestRuntime, sa: Uuid, human: Uuid) {
    let (s, body) = rt.request_as(Method::POST, &format!("/api/v1/service-accounts/{sa}/act-as"), &rt.token, Some(&json!({ "userId": human }))).await;
    assert_eq!(s, StatusCode::OK, "grant act-as: {body}");
}

/// Insert a user directly with a role carrying exactly `perms`.
/// Infers kind from email: `sa+*` = service, `agent+*` = agent, else human.
async fn db_user(pool: &sqlx::PgPool, email: &str, perms: &[&str]) -> Uuid {
    let uid = Uuid::new_v4();
    let kind = if email.starts_with("sa+") { "service" }
        else if email.starts_with("agent+") { "agent" }
        else { "human" };
    sqlx::query("INSERT INTO rootcx_system.users (id, email, kind) VALUES ($1, $2, $3)")
        .bind(uid).bind(email).bind(kind).execute(pool).await.unwrap();
    grant_perms(pool, uid, perms).await;
    uid
}

// ── Client credentials (RFC 6749 4.4) ────────────────────────────────

#[tokio::test]
async fn sa_client_credentials_issues_usable_token() {
    let rt = harness::TestRuntime::boot().await;
    let (sa, email) = create_sa(&rt, "billing_sync").await;
    let key = create_credential(&rt, sa).await;
    assert!(key.starts_with("rcs_"), "key must carry the rcs_ prefix: {key}");

    let res = token_exchange(&rt, sa, &key).await;
    assert_eq!(res.status(), StatusCode::OK);
    let tok: Value = res.json().await.unwrap();
    assert_eq!(tok["token_type"], "Bearer");
    let access = tok["access_token"].as_str().expect("access_token").to_string();

    // The issued JWT flows through the unchanged Identity extractor.
    let (s, me) = rt.request_as(Method::GET, "/api/v1/auth/me", &access, None).await;
    assert_eq!(s, StatusCode::OK, "{me}");
    assert_eq!(me["email"], email);
    rt.shutdown().await;
}

#[tokio::test]
async fn sa_token_rejects_bad_secret() {
    let rt = harness::TestRuntime::boot().await;
    let (sa, _) = create_sa(&rt, "reporting").await;
    let _ = create_credential(&rt, sa).await;

    assert_eq!(token_status(&rt, sa, "rcs_not_the_real_key").await, StatusCode::UNAUTHORIZED);
    rt.shutdown().await;
}

#[tokio::test]
async fn disabled_sa_token_refused() {
    let rt = harness::TestRuntime::boot().await;
    let (sa, _) = create_sa(&rt, "nightly").await;
    let key = create_credential(&rt, sa).await;

    let (s, _) = rt.request_as(Method::POST, &format!("/api/v1/service-accounts/{sa}/disable"), &rt.token, None).await;
    assert_eq!(s, StatusCode::OK);

    assert_eq!(token_status(&rt, sa, &key).await, StatusCode::UNAUTHORIZED, "disabled SA must not get a token");
    rt.shutdown().await;
}

// ── No interactive login for non-humans ──────────────────────────────

#[tokio::test]
async fn sa_cannot_login_interactively() {
    let rt = harness::TestRuntime::boot().await;
    let (_, email) = create_sa(&rt, "loginless").await;
    let (s, _) = rt.post_unauthed("/api/v1/auth/login", &json!({ "email": email, "password": "anything" })).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "service accounts have no interactive login");
    rt.shutdown().await;
}

// ── The single act-as gate: delegation + anti-escalation ─────────────

#[tokio::test]
async fn act_as_requires_delegation_and_subset() {
    let rt = harness::TestRuntime::boot().await;
    let pool = rt.pool();

    let human = db_user(pool, "narrow@t.local", &["app:crm:contacts.read"]).await;
    let sa_big = db_user(pool, "sa+big@localhost", &["app:crm:*"]).await;
    let sa_small = db_user(pool, "sa+small@localhost", &["app:crm:contacts.read"]).await;

    // No delegation -> denied.
    assert!(rootcx_core::governance::delegation::act_as::assert_can_act_as(pool, human, sa_big).await.is_err(),
        "no act-as delegation must deny");

    // Delegation exists but target exceeds the human -> anti-escalation denies.
    rootcx_core::governance::delegation::act_as::grant(pool, human, sa_big).await.unwrap();
    assert!(rootcx_core::governance::delegation::act_as::assert_can_act_as(pool, human, sa_big).await.is_err(),
        "target perms exceed human -> anti-escalation must deny");

    // Delegation + subset -> allowed.
    rootcx_core::governance::delegation::act_as::grant(pool, human, sa_small).await.unwrap();
    assert!(rootcx_core::governance::delegation::act_as::assert_can_act_as(pool, human, sa_small).await.is_ok(),
        "subset target with delegation must be allowed");

    rt.shutdown().await;
}

#[tokio::test]
async fn disabled_sa_loses_access_immediately() {
    let rt = harness::TestRuntime::boot().await;
    let (sa, _) = create_sa(&rt, "immediate").await;
    let key = create_credential(&rt, sa).await;
    let access = token_access(&rt, sa, &key).await;

    // The live token works...
    let (s, _) = rt.request_as(Method::GET, "/api/v1/auth/me", &access, None).await;
    assert_eq!(s, StatusCode::OK);

    // ...until the SA is disabled, after which the SAME token is denied at once
    // (not at expiry): the Identity extractor re-checks enablement per request.
    let (s, _) = rt.request_as(Method::POST, &format!("/api/v1/service-accounts/{sa}/disable"), &rt.token, None).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = rt.request_as(Method::GET, "/api/v1/auth/me", &access, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "disabled SA must lose access immediately, not at token expiry");
    rt.shutdown().await;
}

#[tokio::test]
async fn run_as_anti_escalation_end_to_end() {
    let rt = harness::TestRuntime::boot().await;

    // A human who may trigger app `test` but holds nothing else.
    let (htok, huid) = human_with(&rt, "narrow-e2e@t.local", &["app:test:invoke"]).await;

    // A SA that EXCEEDS the human (also holds contacts.read).
    let (sa_big, _) = create_sa(&rt, "bigsa").await;
    grant_perms(rt.pool(), sa_big, &["app:test:invoke", "app:test:contacts.read"]).await;
    grant_act_as(&rt, sa_big, huid).await;

    // Owning a job as the over-privileged SA is denied by anti-escalation.
    let (s, body) = rt.request_as(
        Method::POST, "/api/v1/apps/test/jobs", &htok,
        Some(&json!({ "user_id": sa_big.to_string(), "payload": {} })),
    ).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "run_as a SA that exceeds the human must be denied: {body}");

    // A SA that is a SUBSET of the human is allowed.
    let (sa_small, _) = create_sa(&rt, "smallsa").await;
    grant_perms(rt.pool(), sa_small, &["app:test:invoke"]).await;
    grant_act_as(&rt, sa_small, huid).await;
    let (s, body) = rt.request_as(
        Method::POST, "/api/v1/apps/test/jobs", &htok,
        Some(&json!({ "user_id": sa_small.to_string(), "payload": {} })),
    ).await;
    assert_eq!(s, StatusCode::CREATED, "run_as a subset SA must be allowed: {body}");
    rt.shutdown().await;
}

#[tokio::test]
async fn multiple_credentials_rotation() {
    let rt = harness::TestRuntime::boot().await;
    let (sa, _) = create_sa(&rt, "rotate").await;

    let (id1, k1) = create_credential_with_id(&rt, sa).await;
    let (_id2, k2) = create_credential_with_id(&rt, sa).await;

    // Both active keys authenticate (zero-downtime rotation window).
    assert_eq!(token_status(&rt, sa, &k1).await, StatusCode::OK);
    assert_eq!(token_status(&rt, sa, &k2).await, StatusCode::OK);

    // Revoke the old key: it stops, the new one keeps working.
    let s = rt.delete(&format!("/api/v1/service-accounts/{sa}/credentials/{id1}")).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(token_status(&rt, sa, &k1).await, StatusCode::UNAUTHORIZED, "revoked key must stop working");
    assert_eq!(token_status(&rt, sa, &k2).await, StatusCode::OK, "other key must still work");
    rt.shutdown().await;
}

#[tokio::test]
async fn escalate_permission_does_not_bypass_subset_check() {
    let rt = harness::TestRuntime::boot().await;
    let pool = rt.pool();

    // A user holding the old escalate permission must NOT bypass the subset check.
    let admin = db_user(pool, "escalator@t.local", &["admin:rbac.escalate"]).await;
    let sa_big = db_user(pool, "sa+huge@localhost", &["app:crm:*", "app:billing:*"]).await;

    rootcx_core::governance::delegation::act_as::grant(pool, admin, sa_big).await.unwrap();
    assert!(rootcx_core::governance::delegation::act_as::assert_can_act_as(pool, admin, sa_big).await.is_err(),
        "anti-escalation must deny even when caller holds admin:rbac.escalate");
    rt.shutdown().await;
}

// ── Expired credential ───────────────────────────────────────────────

#[tokio::test]
async fn expired_credential_denied() {
    let rt = harness::TestRuntime::boot().await;
    let (sa, _) = create_sa(&rt, "expiry").await;
    let key = create_credential(&rt, sa).await;

    // Manually expire the credential in the DB.
    sqlx::query("UPDATE rootcx_system.sa_credentials SET expires_at = now() - interval '1 hour' WHERE sa_user_id = $1")
        .bind(sa).execute(rt.pool()).await.unwrap();

    assert_eq!(token_status(&rt, sa, &key).await, StatusCode::UNAUTHORIZED,
        "expired credential must be rejected");
    rt.shutdown().await;
}

// ── Non-admin cannot manage SAs ──────────────────────────────────────

#[tokio::test]
async fn non_admin_sa_crud_denied() {
    let rt = harness::TestRuntime::boot().await;
    let (tok, _) = human_with(&rt, "pleb@t.local", &["app:crm:contacts.read"]).await;

    for (method, path) in [
        (Method::GET, "/api/v1/service-accounts"),
        (Method::POST, "/api/v1/service-accounts"),
    ] {
        let (s, _) = rt.request_as(method.clone(), path, &tok, Some(&json!({ "slug": "evil" }))).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{method} {path} must require admin:service_accounts.manage");
    }
    rt.shutdown().await;
}

// ── SA delete cascade ────────────────────────────────────────────────

#[tokio::test]
async fn delete_sa_cascades_credentials_and_delegations() {
    let rt = harness::TestRuntime::boot().await;
    let (sa, _) = create_sa(&rt, "cascade").await;
    let key = create_credential(&rt, sa).await;

    let admin_id: Uuid = sqlx::query_scalar("SELECT id FROM rootcx_system.users WHERE email = 'admin@test.local'")
        .fetch_one(rt.pool()).await.unwrap();
    grant_act_as(&rt, sa, admin_id).await;

    // Pre-condition: credential works, delegation valid.
    assert_eq!(token_status(&rt, sa, &key).await, StatusCode::OK);
    assert!(rootcx_core::governance::delegation::is_valid(rt.pool(), admin_id, sa).await.unwrap());

    // Delete SA.
    let s = rt.delete(&format!("/api/v1/service-accounts/{sa}")).await;
    assert_eq!(s, StatusCode::OK);

    // Post-condition: credential dead, delegation revoked.
    assert_eq!(token_status(&rt, sa, &key).await, StatusCode::UNAUTHORIZED, "credential must die with SA");
    assert!(!rootcx_core::governance::delegation::is_valid(rt.pool(), admin_id, sa).await.unwrap(),
        "delegation must be revoked on SA deletion");
    rt.shutdown().await;
}

// ── Act-as revoked then denied ───────────────────────────────────────

#[tokio::test]
async fn act_as_revoked_denies() {
    let rt = harness::TestRuntime::boot().await;
    let pool = rt.pool();
    let human = db_user(pool, "revtest@t.local", &["app:crm:contacts.read"]).await;
    let sa = db_user(pool, "sa+revtest@localhost", &["app:crm:contacts.read"]).await;

    rootcx_core::governance::delegation::act_as::grant(pool, human, sa).await.unwrap();
    assert!(rootcx_core::governance::delegation::act_as::assert_can_act_as(pool, human, sa).await.is_ok());

    rootcx_core::governance::delegation::act_as::revoke(pool, human, sa).await.unwrap();
    assert!(rootcx_core::governance::delegation::act_as::assert_can_act_as(pool, human, sa).await.is_err(),
        "revoked act-as must deny");
    rt.shutdown().await;
}

// ── Delegation expiry honored ────────────────────────────────────────

#[tokio::test]
async fn expired_delegation_denied() {
    let rt = harness::TestRuntime::boot().await;
    let pool = rt.pool();
    let human = db_user(pool, "dexp@t.local", &[]).await;
    let sa = db_user(pool, "sa+dexp@localhost", &[]).await;

    sqlx::query(
        "INSERT INTO rootcx_system.delegations (delegator_uid, delegatee_uid, trigger_type, expires_at) \
         VALUES ($1, $2, 'act_as', now() - interval '1 hour')")
        .bind(human).bind(sa).execute(pool).await.unwrap();

    assert!(!rootcx_core::governance::delegation::is_valid(pool, human, sa).await.unwrap(),
        "expired delegation must not be valid");
    rt.shutdown().await;
}

// ── Self act-as always allowed (no DB lookup needed) ─────────────────

#[tokio::test]
async fn act_as_self_always_allowed() {
    let rt = harness::TestRuntime::boot().await;
    let pool = rt.pool();
    let uid = db_user(pool, "self@t.local", &[]).await;
    assert!(rootcx_core::governance::delegation::act_as::assert_can_act_as(pool, uid, uid).await.is_ok(),
        "acting as oneself must always pass without delegation");
    rt.shutdown().await;
}

// ── Wildcard anti-escalation (cross-namespace) ───────────────────────

#[tokio::test]
async fn wildcard_perm_no_cross_namespace_escalation() {
    let rt = harness::TestRuntime::boot().await;
    let pool = rt.pool();
    let human = db_user(pool, "apponly@t.local", &["app:crm:*"]).await;
    let sa = db_user(pool, "sa+cross@localhost", &["admin:audit.read"]).await;

    rootcx_core::governance::delegation::act_as::grant(pool, human, sa).await.unwrap();
    assert!(rootcx_core::governance::delegation::act_as::assert_can_act_as(pool, human, sa).await.is_err(),
        "app:crm:* must not satisfy admin:audit.read (cross-namespace escalation)");
    rt.shutdown().await;
}

// ── Slug validation (parameterized) ──────────────────────────────────

#[tokio::test]
async fn invalid_slugs_rejected() {
    let rt = harness::TestRuntime::boot().await;
    let bad_slugs = ["", "AB", &"a".repeat(49), "hello world", "sa;drop", "a/b"];
    for slug in bad_slugs {
        let (s, _) = rt.request_as(
            Method::POST, "/api/v1/service-accounts", &rt.token, Some(&json!({ "slug": slug })),
        ).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "slug '{slug}' must be rejected");
    }
    rt.shutdown().await;
}

// ── Duplicate slug returns 400, not 500 ──────────────────────────────

#[tokio::test]
async fn duplicate_slug_returns_400() {
    let rt = harness::TestRuntime::boot().await;
    create_sa(&rt, "dupetest").await;
    let (s, body) = rt.request_as(
        Method::POST, "/api/v1/service-accounts", &rt.token, Some(&json!({ "slug": "dupetest" })),
    ).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "duplicate slug must be 400: {body}");
    rt.shutdown().await;
}

// ── Token exchange with bad grant_type / non-existent client_id ──────

#[tokio::test]
async fn token_exchange_edge_cases() {
    let rt = harness::TestRuntime::boot().await;
    let (sa, _) = create_sa(&rt, "edges").await;
    let key = create_credential(&rt, sa).await;
    let fake_id = Uuid::new_v4().to_string();
    let cid = sa.to_string();

    let cases: &[(&str, &str, &str, StatusCode, &str)] = &[
        ("password", &cid, &key, StatusCode::BAD_REQUEST, "unsupported grant_type"),
        ("client_credentials", &fake_id, "rcs_fake", StatusCode::UNAUTHORIZED, "unknown client_id"),
    ];
    for &(grant, id, secret, expected, label) in cases {
        let res = rt.client.post(rt.url("/api/v1/auth/token"))
            .form(&[("grant_type", grant), ("client_id", id), ("client_secret", secret)])
            .send().await.unwrap();
        assert_eq!(res.status(), expected, "{label} must be {expected}");
    }
    rt.shutdown().await;
}

// ── Job enqueue without invoke permission ────────────────────────────

#[tokio::test]
async fn job_enqueue_requires_invoke_perm() {
    let rt = harness::TestRuntime::boot().await;
    rt.install("gated", "items").await;
    let (tok, _) = human_with(&rt, "noinvoke@t.local", &["app:gated:items.read"]).await;
    let (s, _) = rt.request_as(
        Method::POST, "/api/v1/apps/gated/jobs", &tok,
        Some(&json!({ "payload": {} })),
    ).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "enqueue without app:gated:invoke must be denied");
    rt.shutdown().await;
}

// ── Per-human act-as isolation ───────────────────────────────────────

#[tokio::test]
async fn act_as_per_human_isolation() {
    let rt = harness::TestRuntime::boot().await;
    let pool = rt.pool();
    let alice = db_user(pool, "alice-iso@t.local", &["app:x:invoke"]).await;
    let bob = db_user(pool, "bob-iso@t.local", &["app:x:invoke"]).await;
    let sa = db_user(pool, "sa+iso@localhost", &["app:x:invoke"]).await;

    rootcx_core::governance::delegation::act_as::grant(pool, alice, sa).await.unwrap();
    assert!(rootcx_core::governance::delegation::act_as::assert_can_act_as(pool, alice, sa).await.is_ok());
    assert!(rootcx_core::governance::delegation::act_as::assert_can_act_as(pool, bob, sa).await.is_err(),
        "Bob has no delegation to SA, must be denied even though Alice does");
    rt.shutdown().await;
}

// ── runAs on cron stores correct owner ───────────────────────────────

#[tokio::test]
async fn cron_run_as_stores_sa_as_owner() {
    let rt = harness::TestRuntime::boot().await;
    rt.install("cronsa", "items").await;
    let pool = rt.pool();

    let (sa, _) = create_sa(&rt, "cronowner").await;
    grant_perms(pool, sa, &["app:cronsa:cron.write"]).await;

    let admin_id: Uuid = sqlx::query_scalar("SELECT id FROM rootcx_system.users WHERE email = 'admin@test.local'")
        .fetch_one(pool).await.unwrap();
    grant_act_as(&rt, sa, admin_id).await;

    let (s, body) = rt.post_json("/api/v1/apps/cronsa/crons", &json!({
        "name": "sa_cron", "schedule": "0 * * * *", "runAs": sa.to_string()
    })).await;
    assert_eq!(s, StatusCode::CREATED, "create cron with runAs: {body}");

    let owner: Uuid = sqlx::query_scalar(
        "SELECT created_by FROM rootcx_system.cron_schedules WHERE app_id = 'cronsa' AND name = 'sa_cron'")
        .fetch_one(pool).await.unwrap();
    assert_eq!(owner, sa, "created_by must be the SA, not the admin");
    rt.shutdown().await;
}
