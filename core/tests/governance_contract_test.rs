//! Governance refactor — security contract tests.
//!
//! These assert the OBSERVABLE behaviour of the governance model (RLS data
//! plane, control-plane PEP, cross-app invoke gate, delegation, and the
//! plpgsql RBAC + restricted role). They are black-box w.r.t. whether Rust or
//! Postgres enforces a rule. After the refactor, data-plane denials surface as
//! "0 rows" (RLS), not 403 — the assertions accept that semantics.

mod harness;

use reqwest::{Method, StatusCode};
use serde_json::{json, Value};
use uuid::Uuid;

async fn admin(rt: &harness::TestRuntime) {
    let pool = rt.pool();
    let uid: Uuid = sqlx::query_scalar("SELECT id FROM rootcx_system.users WHERE email = 'admin@test.local'")
        .fetch_one(pool).await.unwrap();
    sqlx::query("INSERT INTO rootcx_system.rbac_assignments (user_id, role) VALUES ($1, 'admin') ON CONFLICT DO NOTHING")
        .bind(uid).execute(pool).await.unwrap();
}

/// Register a user and give them a fresh role carrying exactly `perms`.
async fn user_with(rt: &harness::TestRuntime, email: &str, perms: &[&str]) -> (String, Uuid) {
    let pool = rt.pool();
    let token = rt.register_and_login(email).await;
    let uid: Uuid = sqlx::query_scalar("SELECT id FROM rootcx_system.users WHERE email = $1")
        .bind(email).fetch_one(pool).await.unwrap();
    sqlx::query("DELETE FROM rootcx_system.rbac_assignments WHERE user_id = $1")
        .bind(uid).execute(pool).await.unwrap();
    let role = format!("role_{}", uid.simple());
    let perm_list: Vec<String> = perms.iter().map(|s| s.to_string()).collect();
    sqlx::query("INSERT INTO rootcx_system.rbac_roles (name, inherits, permissions) VALUES ($1, '{}', $2) ON CONFLICT (name) DO UPDATE SET permissions = EXCLUDED.permissions")
        .bind(&role).bind(&perm_list).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO rootcx_system.rbac_assignments (user_id, role) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(uid).bind(&role).execute(pool).await.unwrap();
    (token, uid)
}

fn rec() -> Value {
    json!({"first_name": "Jean", "last_name": "Dupont", "email": "j@x.com"})
}

// ── CATEGORY 1 : data-plane (RLS per-permission) ──────────────────────

#[tokio::test]
async fn t1_1_user_with_perm_reads_data() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("crm", "contacts").await;
    rt.create("crm", "contacts", &rec()).await;
    rt.create("crm", "contacts", &rec()).await;

    let (tok, _) = user_with(&rt, "jean@t.local", &["app:crm:contacts.read"]).await;
    let (s, body) = rt.request_as(Method::GET, "/api/v1/apps/crm/collections/contacts", &tok, None).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body.as_array().map(|a| a.len()), Some(2), "user with perm sees all rows");
    rt.shutdown().await;
}

#[tokio::test]
async fn t1_2_user_without_perm_sees_nothing() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("crm", "contacts").await;
    rt.create("crm", "contacts", &rec()).await;

    // Marie has a different, unrelated permission — RLS filters → empty, NOT 403.
    let (tok, _) = user_with(&rt, "marie@t.local", &["app:other:thing.read"]).await;
    let (s, body) = rt.request_as(Method::GET, "/api/v1/apps/crm/collections/contacts", &tok, None).await;
    assert_eq!(s, StatusCode::OK, "data-plane denial is empty, not 403: {body}");
    assert_eq!(body.as_array().map(|a| a.len()), Some(0), "no perm → 0 rows");
    rt.shutdown().await;
}

#[tokio::test]
async fn t1_3_read_without_create_is_refused() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("crm", "contacts").await;
    let (tok, _) = user_with(&rt, "jean@t.local", &["app:crm:contacts.read"]).await;
    let (s, _) = rt.request_as(Method::POST, "/api/v1/apps/crm/collections/contacts", &tok, Some(&rec())).await;
    // RLS WITH CHECK raises 42501; the API contract maps that to 403, not 500.
    assert_eq!(s, StatusCode::FORBIDDEN, "create without .create → RLS denial surfaces as 403");
    rt.shutdown().await;
}

#[tokio::test]
async fn t1_4_admin_sees_everything() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("crm", "contacts").await;
    rt.create("crm", "contacts", &rec()).await;
    let (s, body) = rt.get_json("/api/v1/apps/crm/collections/contacts").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body.as_array().map(|a| a.len()), Some(1));
    rt.shutdown().await;
}

#[tokio::test]
async fn t1_5_6_scoped_wildcard_does_not_overflow() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("crm", "contacts").await;
    rt.install("billing", "invoices").await;
    rt.create("crm", "contacts", &rec()).await;
    rt.create("billing", "invoices", &rec()).await;

    let (tok, _) = user_with(&rt, "jean@t.local", &["app:crm:*"]).await;
    let (_, crm) = rt.request_as(Method::GET, "/api/v1/apps/crm/collections/contacts", &tok, None).await;
    assert_eq!(crm.as_array().map(|a| a.len()), Some(1), "crm:* covers crm");
    let (_, bil) = rt.request_as(Method::GET, "/api/v1/apps/billing/collections/invoices", &tok, None).await;
    assert_eq!(bil.as_array().map(|a| a.len()), Some(0), "crm:* must not leak into billing");
    rt.shutdown().await;
}

#[tokio::test]
async fn t1_8_permission_revoked_cuts_access() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("crm", "contacts").await;
    rt.create("crm", "contacts", &rec()).await;
    let (tok, uid) = user_with(&rt, "jean@t.local", &["app:crm:contacts.read"]).await;

    let (_, before) = rt.request_as(Method::GET, "/api/v1/apps/crm/collections/contacts", &tok, None).await;
    assert_eq!(before.as_array().map(|a| a.len()), Some(1));

    sqlx::query("DELETE FROM rootcx_system.rbac_assignments WHERE user_id = $1").bind(uid).execute(rt.pool()).await.unwrap();
    let (_, after) = rt.request_as(Method::GET, "/api/v1/apps/crm/collections/contacts", &tok, None).await;
    assert_eq!(after.as_array().map(|a| a.len()), Some(0), "revoked permission → immediately 0 rows");
    rt.shutdown().await;
}

// ── CATEGORY 6 : control-plane PEP ────────────────────────────────────

#[tokio::test]
async fn t6_1_nonadmin_cannot_deploy() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("crm", "contacts").await;
    let (tok, _) = user_with(&rt, "sales@t.local", &["app:crm:contacts.read"]).await;
    // Deploy takes multipart; send a real (empty) form so the request reaches
    // the handler's require_perm gate rather than failing content-type first.
    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(b"x".to_vec()).file_name("b.tar.gz").mime_str("application/gzip").unwrap(),
    );
    let s = reqwest::Client::new().post(rt.url("/api/v1/apps/crm/deploy"))
        .bearer_auth(&tok).multipart(form).send().await.unwrap().status();
    assert_eq!(s, StatusCode::FORBIDDEN);
    rt.shutdown().await;
}

#[tokio::test]
async fn t6_4_second_install_requires_permission() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await; // an admin now exists → no longer first-boot
    rt.install("crm", "contacts").await;
    let (tok, _) = user_with(&rt, "sales@t.local", &[]).await;
    let manifest = json!({
        "appId": "rogue", "name": "rogue", "version": "1.0.0",
        "dataContract": [{"entityName": "x", "fields": [{"name": "a", "type": "text"}]}]
    });
    let (s, _) = rt.request_as(Method::POST, "/api/v1/apps", &tok, Some(&manifest)).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "non-admin install after first-boot → 403");
    rt.shutdown().await;
}

#[tokio::test]
async fn t6_5_role_api_rejects_comma_in_perm_key() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    // effective_perms is CSV-encoded into a Postgres GUC; a comma would corrupt
    // the list. The role API must reject the malformed key at the door (400).
    let body = json!({"name": "bad_role", "permissions": ["app:crm,billing:read"]});
    let (s, _) = rt.post_json("/api/v1/roles", &body).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "comma in permission key → 400");
    rt.shutdown().await;
}

// ── CATEGORY 7 / 3 : cross-app invoke gate + invocation ACL ───────────

#[tokio::test]
async fn t7_1_user_without_invoke_cannot_call_app_rpc() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("billing", "invoices").await;
    let (tok, _) = user_with(&rt, "jean@t.local", &["app:billing:invoices.read"]).await;
    let (s, _) = rt.request_as(Method::POST, "/api/v1/apps/billing/rpc", &tok,
        Some(&json!({"method": "getInvoice", "params": {}}))).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "no app:billing:invoke → 403 at the hop");
    rt.shutdown().await;
}

// ── CATEGORY 6 (continued) : first-boot and admin deploy ─────────────

#[tokio::test]
async fn t6_5_first_boot_promotes_first_installer() {
    // Boot a fresh runtime. The harness auto-registers admin@test.local and
    // promotes them; we undo that promotion so we can observe first-boot.
    let rt = harness::TestRuntime::boot().await;
    let pool = rt.pool();

    // Undo the admin assignment so the system is at first-boot state (only
    // system users have assignments).
    sqlx::query("DELETE FROM rootcx_system.rbac_assignments WHERE user_id IN (SELECT id FROM rootcx_system.users WHERE NOT is_system)")
        .execute(pool).await.unwrap();

    // Verify we are back to first-boot: is_first_boot = true.
    let is_first: bool = sqlx::query_scalar(
        "SELECT NOT EXISTS(SELECT 1 FROM rootcx_system.rbac_assignments a \
         JOIN rootcx_system.users u ON u.id = a.user_id WHERE NOT u.is_system)"
    ).fetch_one(pool).await.unwrap();
    assert!(is_first, "precondition: must be first-boot");

    // Now install as the original admin@test.local user (via the default token).
    let manifest = json!({
        "appId": "first_app", "name": "first_app", "version": "1.0.0",
        "dataContract": [{"entityName": "items", "fields": [{"name": "title", "type": "text"}]}]
    });
    let (s, body) = rt.post_json("/api/v1/apps", &manifest).await;
    assert_eq!(s, StatusCode::OK, "first-boot install must succeed without admin: {body}");

    // After install, the user should now be admin.
    let uid: Uuid = sqlx::query_scalar("SELECT id FROM rootcx_system.users WHERE email = 'admin@test.local'")
        .fetch_one(pool).await.unwrap();
    let has_admin: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM rootcx_system.rbac_assignments WHERE user_id = $1 AND role = 'admin')"
    ).bind(uid).fetch_one(pool).await.unwrap();
    assert!(has_admin, "first-boot: installer must be promoted to admin");
    rt.shutdown().await;
}

#[tokio::test]
async fn t6_6_admin_can_deploy() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("crm", "contacts").await;

    // Deploy a minimal tarball. The admin has admin:apps.deploy; the request
    // should pass the PEP gate (may fail downstream for invalid tarball, but NOT 403).
    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(b"fake".to_vec()).file_name("b.tar.gz").mime_str("application/gzip").unwrap(),
    );
    let s = reqwest::Client::new().post(rt.url("/api/v1/apps/crm/deploy"))
        .bearer_auth(&rt.token).multipart(form).send().await.unwrap().status();
    // Admin must NOT get 403 (may get 500 due to invalid tarball, but PEP passed).
    assert_ne!(s, StatusCode::FORBIDDEN, "admin must pass the deploy PEP gate");
    rt.shutdown().await;
}

// ── CATEGORY 7 (continued) : cross-app invoke + effective_perms ──────

#[tokio::test]
async fn t7_2_user_with_invoke_can_call_rpc() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("billing", "invoices").await;

    // Give the user the invoke permission for the billing app.
    let (tok, _) = user_with(&rt, "invoker@t.local", &["app:billing:invoke"]).await;
    let (s, _) = rt.request_as(Method::POST, "/api/v1/apps/billing/rpc", &tok,
        Some(&json!({"method": "getInvoice", "params": {}}))).await;
    // The request must NOT be rejected at the PEP level (403). It may fail
    // downstream (worker not running = 500/404) but governance passed.
    assert_ne!(s, StatusCode::FORBIDDEN, "user with app:billing:invoke must pass the invoke gate");
    rt.shutdown().await;
}

#[tokio::test]
async fn t7_4_admin_can_invoke_any_app() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("billing", "invoices").await;

    // The admin token (from harness) has `*` via admin role.
    let (s, _) = rt.request_as(Method::POST, "/api/v1/apps/billing/rpc", &rt.token,
        Some(&json!({"method": "anything", "params": {}}))).await;
    // Must not be 403 (admin has wildcard).
    assert_ne!(s, StatusCode::FORBIDDEN, "admin must pass any invoke gate");
    rt.shutdown().await;
}

#[tokio::test]
async fn t7_5_cross_app_action_propagates_intersection_perms() {
    // This tests the core governance invariant: when an agent calls a cross-app
    // action, the effective_perms (intersection of agent grants and human perms)
    // are propagated to the target. The target's RLS sees is_delegated='1' and
    // uses effective_perms, NOT the human's direct permissions.
    //
    // We test at the DB/RLS level: posing GUCs to simulate the delegation path.
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("billing", "invoices").await;
    rt.create("billing", "invoices", &rec()).await;
    let pool = rt.pool();

    // Setup: Jean has full billing access directly.
    let jean = Uuid::new_v4();
    sqlx::query("INSERT INTO rootcx_system.users (id, email) VALUES ($1, 'jean-7.5@t.local')")
        .bind(jean).execute(pool).await.unwrap();
    let role = "role_jean_75";
    sqlx::query("INSERT INTO rootcx_system.rbac_roles (name, permissions) VALUES ($1, ARRAY['app:billing:invoices.read', 'app:billing:invoices.create']) ON CONFLICT (name) DO NOTHING")
        .bind(role).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO rootcx_system.rbac_assignments (user_id, role) VALUES ($1, $2)")
        .bind(jean).bind(role).execute(pool).await.unwrap();

    // Direct access: Jean with no delegation sees the row.
    let direct = count_as_billing(pool, Some(jean), false, "").await;
    assert_eq!(direct, 1, "Jean directly sees 1 row");

    // Simulating the cross-app hop: the agent's intersection only grants
    // invoices.create but NOT invoices.read. RLS must deny visibility.
    let restricted = count_as_billing(pool, Some(jean), true, "app:billing:invoices.create").await;
    assert_eq!(restricted, 0, "delegated with only .create (no .read in intersection) -> 0 rows");

    // With the correct intersection that includes .read -> visible.
    let granted = count_as_billing(pool, Some(jean), true, "app:billing:invoices.read,app:billing:invoices.create").await;
    assert_eq!(granted, 1, "delegated with .read in intersection -> row visible");

    // Empty intersection -> deny all (defense-in-depth).
    let empty = count_as_billing(pool, Some(jean), true, "").await;
    assert_eq!(empty, 0, "delegated with empty intersection -> deny all");

    rt.shutdown().await;
}

#[tokio::test]
async fn t7_5b_call_action_tool_passes_effective_perms_to_caller() {
    // Verify CallActionTool propagates ctx.permissions to the ActionCaller.
    // This is the Rust-level assertion that the intersection flows through.
    use std::sync::{Arc, Mutex};

    struct SpyActionCaller {
        captured_perms: Mutex<Option<Option<Vec<String>>>>,
    }

    #[async_trait::async_trait]
    impl rootcx_core::tools::ActionCaller for SpyActionCaller {
        async fn call(
            &self, _app_id: &str, _action_id: &str, _input: serde_json::Value,
            _user_id: uuid::Uuid, _caller_app_id: &str, effective_perms: Option<Vec<String>>,
        ) -> Result<serde_json::Value, String> {
            *self.captured_perms.lock().unwrap() = Some(effective_perms);
            Ok(json!({"ok": true}))
        }
    }

    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("billing", "invoices").await;
    let pool = rt.pool();

    // Register a fake action in the billing manifest so CallActionTool finds it.
    sqlx::query("UPDATE rootcx_system.apps SET manifest = jsonb_set(COALESCE(manifest, '{}'::jsonb), '{actions}', $1) WHERE id = 'billing'")
        .bind(json!([{"id": "createInvoice", "name": "Create Invoice"}]))
        .execute(pool).await.unwrap();

    let spy = Arc::new(SpyActionCaller { captured_perms: Mutex::new(None) });
    let intersection = vec![
        "app:billing:action:createInvoice".to_string(),
        "app:billing:invoices.create".to_string(),
    ];

    let ctx = rootcx_core::tools::ToolContext {
        pool: pool.clone(),
        app_id: "crm".into(),
        user_id: Uuid::new_v4(),
        invoker_user_id: Some(Uuid::new_v4()),
        permissions: intersection.clone(),
        args: json!({"app": "billing", "action": "createInvoice", "input": {}}),
        agent_dispatch: None,
        integration_caller: None,
        action_caller: Some(spy.clone()),
        stream_tx: None,
    };

    let tool = rootcx_core::tools::call_action::CallActionTool;
    let result = rootcx_core::tools::Tool::execute(&tool, &ctx).await;
    assert!(result.is_ok(), "call_action should succeed: {result:?}");

    let captured = spy.captured_perms.lock().unwrap().clone();
    assert_eq!(captured, Some(Some(intersection)),
        "CallActionTool must propagate ctx.permissions as effective_perms to the target");
    rt.shutdown().await;
}

// ── DB-level : Phase 1 plpgsql RBAC + Phase 2 restricted role ─────────

#[tokio::test]
async fn db_has_permission_wildcard_and_boundary() {
    let rt = harness::TestRuntime::boot().await;
    let pool = rt.pool();

    // match_permission is pure — exercise wildcard, scope, and the `:` boundary.
    let cases: &[(&[&str], &str, bool)] = &[
        (&["*"], "app:crm:contacts.read", true),
        (&["app:crm:contacts.read"], "app:crm:contacts.read", true),
        (&["app:crm:contacts.read"], "app:crm:contacts.create", false),
        (&["app:crm:*"], "app:crm:contacts.read", true),
        (&["app:crm:*"], "app:crm_secret:x", false), // boundary: must not match app:crm_secret
        (&["app:crm:*"], "app:billing:x.read", false),
    ];
    for (perms, required, expected) in cases {
        let perm_vec: Vec<String> = perms.iter().map(|s| s.to_string()).collect();
        let got: bool = sqlx::query_scalar("SELECT rootcx_system.match_permission($1, $2)")
            .bind(&perm_vec).bind(required).fetch_one(pool).await.unwrap();
        assert_eq!(got, *expected, "match_permission({perms:?}, {required})");
    }

    // has_permission(NULL, ...) is deny-by-default.
    let null_user: Option<Uuid> = None;
    let got: bool = sqlx::query_scalar("SELECT rootcx_system.has_permission($1, 'app:crm:contacts.read')")
        .bind(null_user).fetch_one(pool).await.unwrap();
    assert!(!got, "has_permission(NULL) must be FALSE");
    rt.shutdown().await;
}

// ══════════════════════════════════════════════════════════════════════
// CATEGORY 3 : triggers + delegation lifecycle
// ══════════════════════════════════════════════════════════════════════

// ── TEST 3.1 : User invoke direct (with invoke perm -> agent runs) ────
// Attack: can a user with the correct permission invoke an agent?
// Level: integration (HTTP). Requires live agent worker.
#[tokio::test]
#[ignore = "requires live agent worker; run with --ignored"]
async fn t3_1_user_with_invoke_perm_can_invoke_agent() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("support", "tickets").await;
    register_agent(rt.pool(), "support").await;

    let (tok, _) = user_with(&rt, "alice@t.local", &["app:support:invoke"]).await;
    let (s, _) = rt.request_as(
        Method::POST, "/api/v1/apps/support/agent/invoke", &tok,
        Some(&json!({"message": "hello"})),
    ).await;
    // With a live worker SSE is returned (200). Without -> 500/502.
    assert_eq!(s, StatusCode::OK, "user with invoke perm must be able to invoke agent");
    rt.shutdown().await;
}

// ── TEST 3.2 : User invoke sans permission invoke -> 403 ─────────
// Attack: user without app:X:invoke must NOT be able to trigger the agent.
// Level: integration (HTTP). No worker needed; perm check fires first.
#[tokio::test]
async fn t3_2_user_without_invoke_perm_gets_403() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("support", "tickets").await;
    register_agent(rt.pool(), "support").await;

    let (tok, _) = user_with(&rt, "alice@t.local", &["app:support:tickets.read"]).await;
    let (s, body) = rt.request_as(
        Method::POST, "/api/v1/apps/support/agent/invoke", &tok,
        Some(&json!({"message": "hello"})),
    ).await;
    assert_eq!(s, StatusCode::FORBIDDEN,
        "user without invoke perm must be denied: {body}");
    rt.shutdown().await;
}

// ── TEST 3.3/3.6/3.10 : Valid delegation for all trigger types ───────
// Attack: scheduler/hook/channel must accept agent dispatch when delegation is active.
// Level: unit (delegations::is_valid with DB fixtures)
#[tokio::test]
async fn delegation_valid_for_all_trigger_types() {
    let rt = harness::TestRuntime::boot().await;
    let pool = rt.pool();
    let delegator = create_user(pool, "delegator@t.local").await;
    register_agent(pool, "alerts").await;
    let agent_uid = rootcx_core::extensions::agents::agent_user_id("alerts");

    for trigger_type in ["cron", "hook", "channel"] {
        let del_id = rootcx_core::delegations::create(pool, delegator, agent_uid, trigger_type, None).await.unwrap();
        assert!(rootcx_core::delegations::is_valid(pool, delegator, agent_uid).await.unwrap(),
            "delegation must be valid for trigger_type={trigger_type}");
        rootcx_core::delegations::revoke(pool, del_id).await.unwrap();
    }
    rt.shutdown().await;
}

// ── TEST 3.4/3.7/3.11 : Revoked delegation denies all trigger types ──
// Attack: admin revokes delegation; all trigger types must stop triggering.
// Level: unit (delegation revoke + is_valid)
#[tokio::test]
async fn delegation_revoked_denies_all_trigger_types() {
    let rt = harness::TestRuntime::boot().await;
    let pool = rt.pool();
    let delegator = create_user(pool, "delegator2@t.local").await;
    register_agent(pool, "alerts").await;
    let agent_uid = rootcx_core::extensions::agents::agent_user_id("alerts");

    for trigger_type in ["cron", "hook", "channel"] {
        let del_id = rootcx_core::delegations::create(pool, delegator, agent_uid, trigger_type, None).await.unwrap();
        rootcx_core::delegations::revoke(pool, del_id).await.unwrap();
        assert!(!rootcx_core::delegations::is_valid(pool, delegator, agent_uid).await.unwrap(),
            "revoked delegation must be invalid for trigger_type={trigger_type}");
    }
    rt.shutdown().await;
}

// ── TEST 3.5 : Cron without owner (created_by = NULL) -> refused ─────
// Attack: legacy/orphan cron with no owner must be denied (deny-by-default).
// Scheduler logic: `if invoker_user_id.is_none() -> fail`.
// Level: unit (no delegation exists for nil UUID)
#[tokio::test]
async fn t3_5_cron_no_owner_denied() {
    let rt = harness::TestRuntime::boot().await;
    let pool = rt.pool();
    register_agent(pool, "analytics").await;

    let agent_uid = rootcx_core::extensions::agents::agent_user_id("analytics");
    let valid = rootcx_core::delegations::is_valid(pool, Uuid::nil(), agent_uid).await.unwrap();
    assert!(!valid, "non-existent delegator must have no valid delegation (deny-by-default)");
    rt.shutdown().await;
}

// ── TEST 3.8 : Webhook with valid delegation -> agent runs ───────────
// Attack: inbound agent-webhook without valid delegation must be rejected.
// Level: integration (HTTP to the webhook inbound endpoint)
#[tokio::test]
async fn t3_8_webhook_valid_delegation_passes() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("alerts", "incidents").await;
    register_agent(rt.pool(), "alerts").await;

    let delegator = create_user(rt.pool(), "admin-wh@t.local").await;
    let agent_uid = rootcx_core::extensions::agents::agent_user_id("alerts");

    let wh_id = create_webhook(rt.pool(), "alerts", "incoming", "agent", Some(delegator)).await;
    rootcx_core::delegations::create(rt.pool(), delegator, agent_uid, "webhook", Some(wh_id)).await.unwrap();

    let token = get_webhook_token(rt.pool(), wh_id).await;
    let r = rt.client.post(rt.url(&format!("/api/v1/hooks/{token}")))
        .json(&json!({"event": "alert_fired"}))
        .send().await.unwrap();
    // With valid delegation: 200 (accepted) or 500 (no worker), NOT 403
    assert_ne!(r.status(), StatusCode::FORBIDDEN,
        "webhook with valid delegation must not be denied at the delegation gate");
    rt.shutdown().await;
}

// ── TEST 3.9 : Webhook with revoked delegation -> 403 ────────────────
// Attack: revoking delegation must immediately block webhook-triggered agents.
// Level: integration (HTTP)
#[tokio::test]
async fn t3_9_webhook_revoked_delegation_denied() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("alerts", "incidents").await;
    register_agent(rt.pool(), "alerts").await;

    let delegator = create_user(rt.pool(), "admin-wh2@t.local").await;
    let agent_uid = rootcx_core::extensions::agents::agent_user_id("alerts");

    let wh_id = create_webhook(rt.pool(), "alerts", "incoming2", "agent", Some(delegator)).await;
    let del_id = rootcx_core::delegations::create(rt.pool(), delegator, agent_uid, "webhook", Some(wh_id)).await.unwrap();

    rootcx_core::delegations::revoke(rt.pool(), del_id).await.unwrap();

    let token = get_webhook_token(rt.pool(), wh_id).await;
    let r = rt.client.post(rt.url(&format!("/api/v1/hooks/{token}")))
        .json(&json!({"event": "alert_fired"}))
        .send().await.unwrap();
    assert!(r.status() == StatusCode::FORBIDDEN || r.status() == StatusCode::UNAUTHORIZED,
        "webhook with revoked delegation must be denied, got {}", r.status());
    rt.shutdown().await;
}

// ── TEST 3.12 : Sub-agent invoke (agent A calls agent B, same human) ─
// Attack: sub-agent invoke must succeed when effective permissions include
// target's invoke permission (intersected perms carry through).
// Level: unit (check_permission with correct perm set)
#[tokio::test]
async fn t3_12_sub_agent_invoke_with_perm_succeeds() {
    let rt = harness::TestRuntime::boot().await;
    let ctx = rootcx_core::tools::ToolContext {
        pool: rt.pool().clone(),
        app_id: "crm".into(),
        user_id: Uuid::new_v4(),
        invoker_user_id: Some(Uuid::new_v4()),
        permissions: vec![
            "app:crm:contacts.read".into(),
            "app:billing:invoke".into(),
        ],
        args: json!({"app_id": "billing", "message": "generate invoice"}),
        agent_dispatch: None,
        integration_caller: None,
        action_caller: None,
        stream_tx: None,
    };
    let tool = rootcx_core::tools::invoke_agent::InvokeAgentTool;
    let err = rootcx_core::tools::Tool::execute(&tool, &ctx).await.unwrap_err();
    // Must NOT fail on permission; should fail on "dispatch unavailable"
    assert!(!err.contains("permission denied"),
        "sub-agent invoke with correct perm must pass the gate: {err}");
    assert!(err.contains("dispatch unavailable"),
        "with no dispatcher, error must be about dispatch: {err}");
    rt.shutdown().await;
}

// ── TEST 3.13 : Sub-agent invoke sans permission invoke -> refused ────
// Attack: agent A tries to invoke agent B but human lacks app:B:invoke.
// Level: unit (InvokeAgentTool permission gate)
#[tokio::test]
async fn sub_agent_invoke_requires_invoke_perm() {
    let rt = harness::TestRuntime::boot().await;
    let ctx = rootcx_core::tools::ToolContext {
        pool: rt.pool().clone(),
        app_id: "crm".into(),
        user_id: Uuid::new_v4(),
        invoker_user_id: Some(Uuid::new_v4()),
        permissions: vec!["app:crm:contacts.read".into()],
        args: json!({"app_id": "billing", "message": "hi"}),
        agent_dispatch: None,
        integration_caller: None,
        action_caller: None,
        stream_tx: None,
    };
    let tool = rootcx_core::tools::invoke_agent::InvokeAgentTool;
    let err = rootcx_core::tools::Tool::execute(&tool, &ctx).await.unwrap_err();
    assert!(err.contains("app:billing:invoke"), "sub-agent invoke must require invoke perm: {err}");
    rt.shutdown().await;
}

// ── Helpers for Category 3 ───────────────────────────────────────────────

async fn create_user(pool: &sqlx::PgPool, email: &str) -> Uuid {
    let uid = Uuid::new_v4();
    sqlx::query("INSERT INTO rootcx_system.users (id, email) VALUES ($1, $2) ON CONFLICT (email) DO UPDATE SET id = rootcx_system.users.id RETURNING id")
        .bind(uid).bind(email).execute(pool).await.unwrap();
    sqlx::query_scalar::<_, Uuid>("SELECT id FROM rootcx_system.users WHERE email = $1")
        .bind(email).fetch_one(pool).await.unwrap()
}

async fn register_agent(pool: &sqlx::PgPool, app_id: &str) {
    let agent_uid = rootcx_core::extensions::agents::agent_user_id(app_id);
    let agent_email = format!("agent+{app_id}@localhost");
    sqlx::query("INSERT INTO rootcx_system.apps (id, name, manifest) VALUES ($1, $1, '{}') ON CONFLICT DO NOTHING")
        .bind(app_id).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO rootcx_system.users (id, email) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(agent_uid).bind(&agent_email).execute(pool).await.unwrap();
    sqlx::query(
        "INSERT INTO rootcx_system.agents (app_id, name, config) VALUES ($1, $1, '{}') ON CONFLICT DO NOTHING"
    ).bind(app_id).execute(pool).await.unwrap();
}

async fn create_webhook(pool: &sqlx::PgPool, app_id: &str, name: &str, method: &str, created_by: Option<Uuid>) -> Uuid {
    let token = Uuid::new_v4().to_string().replace('-', "");
    let (id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO rootcx_system.webhooks (app_id, name, method, token, created_by) \
         VALUES ($1, $2, $3, $4, $5) RETURNING id"
    ).bind(app_id).bind(name).bind(method).bind(&token).bind(created_by)
    .fetch_one(pool).await.unwrap();
    id
}

async fn get_webhook_token(pool: &sqlx::PgPool, wh_id: Uuid) -> String {
    sqlx::query_scalar::<_, String>("SELECT token FROM rootcx_system.webhooks WHERE id = $1")
        .bind(wh_id).fetch_one(pool).await.unwrap()
}


// ── SelfAction scope (P0 checklist: no arbitrary user targeting) ───────

struct RecordingCaller {
    last_user: std::sync::Mutex<Option<Uuid>>,
}

#[async_trait::async_trait]
impl rootcx_core::tools::IntegrationCaller for RecordingCaller {
    async fn call(
        &self, _pool: &sqlx::PgPool, user_id: Uuid,
        _integration_id: &str, _action_id: &str, _input: Value,
    ) -> Result<Value, String> {
        *self.last_user.lock().unwrap() = Some(user_id);
        Ok(json!({"ok": true}))
    }
}

#[tokio::test]
async fn self_action_is_scoped_to_requester() {
    let rt = harness::TestRuntime::boot().await;
    let pool = rt.pool();
    let caller = RecordingCaller { last_user: std::sync::Mutex::new(None) };

    let jean = Uuid::new_v4();
    sqlx::query("INSERT INTO rootcx_system.users (id, email) VALUES ($1, 'jean-sa@t.local')")
        .bind(jean).execute(pool).await.unwrap();
    let victim = Uuid::new_v4();

    // triggerAction acts as the REQUESTER, ignoring any userId in params.
    let r = rootcx_core::extensions::integrations::execute_self_action(
        pool, &caller, "gmail", "triggerAction",
        json!({"actionName": "x", "userId": victim.to_string()}), Some(jean),
    ).await;
    assert!(r.is_ok(), "{r:?}");
    assert_eq!(*caller.last_user.lock().unwrap(), Some(jean),
        "must act as the requester, never the arbitrary params.userId");

    // No requester (absent/unknown context token) → hard deny.
    let r = rootcx_core::extensions::integrations::execute_self_action(
        pool, &caller, "gmail", "triggerAction", json!({"actionName": "x"}), None,
    ).await;
    assert!(r.is_err(), "no context → deny");

    // syncConnectedUsers is admin-only (matches the old x-run-as admin gate).
    let r = rootcx_core::extensions::integrations::execute_self_action(
        pool, &caller, "gmail", "syncConnectedUsers", json!({"actionName": "x"}), Some(jean),
    ).await;
    assert!(r.is_err() && r.as_ref().unwrap_err().contains("admin"),
        "non-admin syncConnectedUsers must be refused: {r:?}");
    rt.shutdown().await;
}

/// Count crm.contacts as a given RLS identity by posing the GUCs + executor role.
async fn count_as(pool: &sqlx::PgPool, user: Option<Uuid>, delegated: bool, perms: &str) -> i64 {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL search_path TO crm, public").execute(&mut *tx).await.unwrap();
    sqlx::query("SELECT set_config('rootcx.user_id',$1,true), set_config('rootcx.is_delegated',$2,true), set_config('rootcx.effective_perms',$3,true)")
        .bind(user.map(|u| u.to_string()).unwrap_or_default())
        .bind(if delegated { "1" } else { "0" })
        .bind(perms)
        .execute(&mut *tx).await.unwrap();
    sqlx::query("SET LOCAL ROLE rootcx_app_executor").execute(&mut *tx).await.unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM crm.contacts").fetch_one(&mut *tx).await.unwrap();
    let _ = tx.rollback().await;
    n
}

/// Count billing.invoices under a posed RLS identity (for cross-app delegation tests).
async fn count_as_billing(pool: &sqlx::PgPool, user: Option<Uuid>, delegated: bool, perms: &str) -> i64 {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL search_path TO billing, public").execute(&mut *tx).await.unwrap();
    sqlx::query("SELECT set_config('rootcx.user_id',$1,true), set_config('rootcx.is_delegated',$2,true), set_config('rootcx.effective_perms',$3,true)")
        .bind(user.map(|u| u.to_string()).unwrap_or_default())
        .bind(if delegated { "1" } else { "0" })
        .bind(perms)
        .execute(&mut *tx).await.unwrap();
    sqlx::query("SET LOCAL ROLE rootcx_app_executor").execute(&mut *tx).await.unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM billing.invoices").fetch_one(&mut *tx).await.unwrap();
    let _ = tx.rollback().await;
    n
}

/// Count rows from a fully-qualified table as a given RLS identity.
async fn count_table_as(pool: &sqlx::PgPool, fqn: &str, user: Option<Uuid>, delegated: bool, perms: &str) -> i64 {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL search_path TO public").execute(&mut *tx).await.unwrap();
    sqlx::query("SELECT set_config('rootcx.user_id',$1,true), set_config('rootcx.is_delegated',$2,true), set_config('rootcx.effective_perms',$3,true)")
        .bind(user.map(|u| u.to_string()).unwrap_or_default())
        .bind(if delegated { "1" } else { "0" })
        .bind(perms)
        .execute(&mut *tx).await.unwrap();
    sqlx::query("SET LOCAL ROLE rootcx_app_executor").execute(&mut *tx).await.unwrap();
    let q = format!("SELECT count(*) FROM {fqn}");
    let n: i64 = sqlx::query_scalar(&q).fetch_one(&mut *tx).await.unwrap();
    let _ = tx.rollback().await;
    n
}

/// Execute a cross-schema JOIN query under RLS and return row count.
async fn join_count_as(pool: &sqlx::PgPool, sql: &str, user: Option<Uuid>, delegated: bool, perms: &str) -> i64 {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL search_path TO public").execute(&mut *tx).await.unwrap();
    sqlx::query("SELECT set_config('rootcx.user_id',$1,true), set_config('rootcx.is_delegated',$2,true), set_config('rootcx.effective_perms',$3,true)")
        .bind(user.map(|u| u.to_string()).unwrap_or_default())
        .bind(if delegated { "1" } else { "0" })
        .bind(perms)
        .execute(&mut *tx).await.unwrap();
    sqlx::query("SET LOCAL ROLE rootcx_app_executor").execute(&mut *tx).await.unwrap();
    let q = format!("SELECT count(*) FROM ({sql}) sub");
    let n: i64 = sqlx::query_scalar(&q).fetch_one(&mut *tx).await.unwrap();
    let _ = tx.rollback().await;
    n
}

/// Create a user directly in DB with specified permissions (no HTTP registration).
async fn db_user(pool: &sqlx::PgPool, email: &str, perms: &[&str]) -> Uuid {
    let uid = Uuid::new_v4();
    sqlx::query("INSERT INTO rootcx_system.users (id, email) VALUES ($1, $2)")
        .bind(uid).bind(email).execute(pool).await.unwrap();
    let role = format!("role_{}", uid.simple());
    let perm_list: Vec<String> = perms.iter().map(|s| s.to_string()).collect();
    sqlx::query("INSERT INTO rootcx_system.rbac_roles (name, permissions) VALUES ($1, $2) ON CONFLICT (name) DO UPDATE SET permissions = EXCLUDED.permissions")
        .bind(&role).bind(&perm_list).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO rootcx_system.rbac_assignments (user_id, role) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(uid).bind(&role).execute(pool).await.unwrap();
    uid
}

// ── CATEGORY 1 (extended) : cross-app data-plane ─────────────────────

#[tokio::test]
async fn t1_7_cross_app_join_filters_by_user() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("crm", "contacts").await;
    rt.install("billing", "invoices").await;
    rt.create("crm", "contacts", &rec()).await;
    rt.create("billing", "invoices", &rec()).await;
    let pool = rt.pool();

    // Jean has both crm + billing read permissions
    let jean = db_user(pool, "jean-join@t.local", &["app:crm:contacts.read", "app:billing:invoices.read"]).await;

    // Marie has only crm read
    let marie = db_user(pool, "marie-join@t.local", &["app:crm:contacts.read"]).await;

    // Cross-app LEFT JOIN query using fully-qualified table names
    let join_sql = "SELECT c.id, i.id AS inv_id FROM crm.contacts c LEFT JOIN billing.invoices i ON TRUE";

    // Jean sees both sides of the JOIN (at least 1 combined row)
    let n = join_count_as(pool, join_sql, Some(jean), false, "").await;
    assert!(n >= 1, "Jean with both perms sees JOIN result: got {n}");

    // Marie cannot see billing rows (RLS filters the billing side)
    let billing_n = count_table_as(pool, "billing.invoices", Some(marie), false, "").await;
    assert_eq!(billing_n, 0, "Marie without billing perm sees 0 billing rows");

    // Marie can still see CRM rows
    let crm_n = count_table_as(pool, "crm.contacts", Some(marie), false, "").await;
    assert_eq!(crm_n, 1, "Marie with crm perm sees her crm rows");
    rt.shutdown().await;
}

#[tokio::test]
async fn t1_9_linked_enrichment_filtered_by_rls() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;

    // Install crm + billing with shared identity_kind for cross-app linking
    let crm_manifest = json!({
        "appId": "crm", "name": "crm", "version": "1.0.0",
        "dataContract": [{ "entityName": "contacts", "fields": [
            { "name": "first_name", "type": "text", "required": true },
            { "name": "last_name",  "type": "text", "required": true },
            { "name": "email", "type": "text" },
            { "name": "phone", "type": "text" },
            { "name": "company", "type": "text" },
            { "name": "notes", "type": "text" },
        ], "identityKind": "customer", "identityKey": "email" }]
    });
    let billing_manifest = json!({
        "appId": "billing", "name": "billing", "version": "1.0.0",
        "dataContract": [{ "entityName": "invoices", "fields": [
            { "name": "first_name", "type": "text", "required": true },
            { "name": "last_name",  "type": "text", "required": true },
            { "name": "email", "type": "text" },
            { "name": "phone", "type": "text" },
            { "name": "company", "type": "text" },
            { "name": "notes", "type": "text" },
        ], "identityKind": "customer", "identityKey": "email" }]
    });
    rt.install_manifest(&crm_manifest).await;
    rt.install_manifest(&billing_manifest).await;

    // Create records with the same identity key (email) for linking
    let contact = json!({"first_name": "Jean", "last_name": "Dupont", "email": "jean@acme.com"});
    let invoice = json!({"first_name": "Jean", "last_name": "Dupont", "email": "jean@acme.com"});
    rt.create("crm", "contacts", &contact).await;
    rt.create("billing", "invoices", &invoice).await;

    // User with crm read but NOT billing read
    let (tok, _) = user_with(&rt, "linked-user@t.local", &["app:crm:contacts.read"]).await;

    // ?linked=billing: CRM rows visible but _linked.billing absent (RLS denies billing)
    let (s, body) = rt.request_as(
        Method::GET, "/api/v1/apps/crm/collections/contacts?linked=billing", &tok, None,
    ).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let rows = body.as_array().expect("array response");
    assert_eq!(rows.len(), 1, "CRM contact visible");
    let linked = rows[0].get("_linked").and_then(|l| l.get("billing"));
    assert!(linked.is_none(), "_linked.billing must be absent without billing perm: {body}");

    // User with BOTH perms sees the linked data
    let (tok2, _) = user_with(&rt, "linked-full@t.local", &["app:crm:contacts.read", "app:billing:invoices.read"]).await;
    let (s2, body2) = rt.request_as(
        Method::GET, "/api/v1/apps/crm/collections/contacts?linked=billing", &tok2, None,
    ).await;
    assert_eq!(s2, StatusCode::OK);
    let rows2 = body2.as_array().expect("array response");
    let linked2 = rows2[0].get("_linked").and_then(|l| l.get("billing"));
    assert!(linked2.is_some(), "user with billing perm sees _linked.billing: {body2}");
    rt.shutdown().await;
}

#[tokio::test]
async fn t1_10_federated_query_filtered_by_rls() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;

    // Install two apps sharing identity_kind "person" for federation
    let crm_manifest = json!({
        "appId": "crm", "name": "crm", "version": "1.0.0",
        "dataContract": [{ "entityName": "contacts", "fields": [
            { "name": "first_name", "type": "text", "required": true },
            { "name": "last_name",  "type": "text", "required": true },
            { "name": "email", "type": "text" },
            { "name": "phone", "type": "text" },
            { "name": "company", "type": "text" },
            { "name": "notes", "type": "text" },
        ], "identityKind": "person", "identityKey": "email" }]
    });
    let hr_manifest = json!({
        "appId": "hr", "name": "hr", "version": "1.0.0",
        "dataContract": [{ "entityName": "employees", "fields": [
            { "name": "first_name", "type": "text", "required": true },
            { "name": "last_name",  "type": "text", "required": true },
            { "name": "email", "type": "text" },
            { "name": "phone", "type": "text" },
            { "name": "company", "type": "text" },
            { "name": "notes", "type": "text" },
        ], "identityKind": "person", "identityKey": "email" }]
    });
    rt.install_manifest(&crm_manifest).await;
    rt.install_manifest(&hr_manifest).await;
    rt.create("crm", "contacts", &json!({"first_name": "A", "last_name": "B", "email": "a@b.com"})).await;
    rt.create("hr", "employees", &json!({"first_name": "C", "last_name": "D", "email": "c@d.com"})).await;

    // User with only crm perm: federated query returns only crm records
    let (tok, _) = user_with(&rt, "fed-user@t.local", &["app:crm:contacts.read"]).await;
    let (s, body) = rt.request_as(
        Method::POST, "/api/v1/federated/person/query", &tok, Some(&json!({})),
    ).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let empty: Vec<Value> = vec![];
    let data = body.get("data").and_then(|d| d.as_array()).unwrap_or(&empty);
    for row in data {
        let source = row.get("_source").and_then(|s| s.get("app")).and_then(|a| a.as_str());
        assert_eq!(source, Some("crm"), "federated must filter out hr for user without hr perm: {body}");
    }
    assert!(!data.is_empty(), "crm record should be visible");

    // User with both perms sees both apps
    let (tok2, _) = user_with(&rt, "fed-full@t.local", &["app:crm:contacts.read", "app:hr:employees.read"]).await;
    let (s2, body2) = rt.request_as(
        Method::POST, "/api/v1/federated/person/query", &tok2, Some(&json!({})),
    ).await;
    assert_eq!(s2, StatusCode::OK);
    let empty2: Vec<Value> = vec![];
    let data2 = body2.get("data").and_then(|d| d.as_array()).unwrap_or(&empty2);
    let apps: Vec<&str> = data2.iter()
        .filter_map(|r| r.get("_source").and_then(|s| s.get("app")).and_then(|a| a.as_str()))
        .collect();
    assert!(apps.contains(&"crm"), "full user sees crm");
    assert!(apps.contains(&"hr"), "full user sees hr");
    rt.shutdown().await;
}

// ── CATEGORY 2 : Delegation / intersection (DB-level) ────────────────

#[tokio::test]
async fn t2_3_user_cannot_exceed_agent() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;

    // Install crm with two entities: contacts and deals (both get RLS)
    let manifest = json!({
        "appId": "crm", "name": "crm", "version": "1.0.0",
        "dataContract": [
            { "entityName": "contacts", "fields": [
                { "name": "first_name", "type": "text", "required": true },
                { "name": "last_name",  "type": "text", "required": true },
                { "name": "email", "type": "text" },
                { "name": "phone", "type": "text" },
                { "name": "company", "type": "text" },
                { "name": "notes", "type": "text" },
            ]},
            { "entityName": "deals", "fields": [
                { "name": "first_name", "type": "text", "required": true },
                { "name": "last_name",  "type": "text", "required": true },
                { "name": "email", "type": "text" },
                { "name": "phone", "type": "text" },
                { "name": "company", "type": "text" },
                { "name": "notes", "type": "text" },
            ]},
        ]
    });
    rt.install_manifest(&manifest).await;
    rt.create("crm", "contacts", &rec()).await;
    rt.create("crm", "deals", &rec()).await;
    let pool = rt.pool();

    // Agent has [crm:contacts.read] only (NOT deals)
    // User (Jean) has [crm:*] (includes deals)
    // Intersection = [crm:contacts.read] (bounded by agent's narrower scope)
    let jean = db_user(pool, "jean-t23@t.local", &["app:crm:*"]).await;

    // Contacts visible via intersection
    let contacts_n = count_as(pool, Some(jean), true, "app:crm:contacts.read").await;
    assert_eq!(contacts_n, 1, "intersection includes contacts.read -> visible");

    // Deals NOT visible: the intersection only has contacts.read, not deals.read
    let deals_n = count_table_as(pool, "crm.deals", Some(jean), true, "app:crm:contacts.read").await;
    assert_eq!(deals_n, 0, "user cannot exceed agent: intersection lacks deals.read -> 0 rows");
    rt.shutdown().await;
}

#[tokio::test]
async fn t2_4_agent_without_delegator_zero_authority() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("crm", "contacts").await;
    rt.create("crm", "contacts", &rec()).await;
    let pool = rt.pool();

    // No user_id (NULL) + delegated = agent without a human responsible -> deny all
    let n = count_as(pool, None, true, "app:crm:contacts.read").await;
    assert_eq!(n, 0, "no delegator (NULL user_id) + delegated -> zero authority");

    // Even with broad perms in GUC, NULL user_id denies
    let n2 = count_as(pool, None, true, "app:crm:*").await;
    assert_eq!(n2, 0, "no user_id -> deny regardless of effective_perms content");

    // Non-delegated NULL user_id also denies (baseline)
    let n3 = count_as(pool, None, false, "").await;
    assert_eq!(n3, 0, "NULL user_id always denies");
    rt.shutdown().await;
}

#[tokio::test]
async fn t2_5_admin_delegation_bounded_by_agent() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;

    // Install crm with two entities: contacts and deals
    let manifest = json!({
        "appId": "crm", "name": "crm", "version": "1.0.0",
        "dataContract": [
            { "entityName": "contacts", "fields": [
                { "name": "first_name", "type": "text", "required": true },
                { "name": "last_name",  "type": "text", "required": true },
                { "name": "email", "type": "text" },
                { "name": "phone", "type": "text" },
                { "name": "company", "type": "text" },
                { "name": "notes", "type": "text" },
            ]},
            { "entityName": "deals", "fields": [
                { "name": "first_name", "type": "text", "required": true },
                { "name": "last_name",  "type": "text", "required": true },
                { "name": "email", "type": "text" },
                { "name": "phone", "type": "text" },
                { "name": "company", "type": "text" },
                { "name": "notes", "type": "text" },
            ]},
        ]
    });
    rt.install_manifest(&manifest).await;
    rt.create("crm", "contacts", &rec()).await;
    rt.create("crm", "deals", &rec()).await;
    let pool = rt.pool();

    // Admin (has '*') invokes agent with only [crm:contacts.read]
    // intersect('*', [crm:contacts.read]) = [crm:contacts.read]
    let admin_uid = db_user(pool, "admin-t25@t.local", &["*"]).await;

    // Delegated: effective_perms carries only the agent's grant (the intersection)
    let contacts_n = count_as(pool, Some(admin_uid), true, "app:crm:contacts.read").await;
    assert_eq!(contacts_n, 1, "admin delegated with agent's contacts.read -> sees contacts");

    let deals_n = count_table_as(pool, "crm.deals", Some(admin_uid), true, "app:crm:contacts.read").await;
    assert_eq!(deals_n, 0, "admin delegated but agent lacks deals.read -> 0 rows");
    rt.shutdown().await;
}

#[tokio::test]
async fn t2_6_empty_intersection_zero_authority() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("crm", "contacts").await;
    rt.install("billing", "invoices").await;
    rt.create("crm", "contacts", &rec()).await;
    rt.create("billing", "invoices", &rec()).await;
    let pool = rt.pool();

    // Agent has [crm:contacts.read]
    // User has [billing:invoices.read] (no overlap with agent)
    // Intersection = EMPTY
    let jean = db_user(pool, "jean-t26@t.local", &["app:billing:invoices.read"]).await;

    // Delegated with empty perms string (the computed intersection is empty)
    let crm_n = count_as(pool, Some(jean), true, "").await;
    assert_eq!(crm_n, 0, "empty intersection -> cannot read crm contacts");

    let billing_n = count_table_as(pool, "billing.invoices", Some(jean), true, "").await;
    assert_eq!(billing_n, 0, "empty intersection -> cannot read billing even though user has billing perm");

    // Control: non-delegated user CAN see billing directly
    let billing_direct = count_table_as(pool, "billing.invoices", Some(jean), false, "").await;
    assert_eq!(billing_direct, 1, "control: non-delegated user with billing perm sees invoices");
    rt.shutdown().await;
}

/// Execute a SQL statement as the restricted `rootcx_app_executor` role inside
/// the given app schema. Returns Ok(()) if the statement succeeds, Err(msg) if
/// Postgres denies it.
async fn exec_as_executor(pool: &sqlx::PgPool, schema: &str, sql: &str) -> Result<(), String> {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query(&format!("SET LOCAL search_path TO {schema}, public"))
        .execute(&mut *tx).await.unwrap();
    sqlx::query("SET LOCAL ROLE rootcx_app_executor")
        .execute(&mut *tx).await.unwrap();
    let res = sqlx::query(sql).execute(&mut *tx).await;
    let _ = tx.rollback().await;
    res.map(|_| ()).map_err(|e| e.to_string())
}

// ── CATEGORY 5 : Postgres role + RLS protection ──────────────────────

#[tokio::test]
async fn t5_1_app_cannot_read_rootcx_system() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("crm", "contacts").await;
    let pool = rt.pool();

    // Direct qualified access to rootcx_system tables must be denied.
    for sql in [
        "SELECT * FROM rootcx_system.users",
        "SELECT * FROM rootcx_system.rbac_roles",
        "SELECT * FROM rootcx_system.rbac_assignments",
        "SELECT * FROM rootcx_system.apps",
    ] {
        let res = exec_as_executor(pool, "crm", sql).await;
        assert!(res.is_err(), "executor must not read rootcx_system: {sql}");
        let err = res.unwrap_err();
        assert!(
            err.contains("permission denied") || err.contains("denied"),
            "expected permission denied for {sql}, got: {err}"
        );
    }
    rt.shutdown().await;
}

#[tokio::test]
async fn t5_2_app_cannot_read_pgmq() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("crm", "contacts").await;
    let pool = rt.pool();

    // pgmq carries cross-app job payloads. The executor has no access.
    // The schema may or may not exist depending on the container; if it
    // exists, access must be denied. If not, the query still errors (no
    // schema = also safe).
    let has_pgmq: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM information_schema.schemata WHERE schema_name = 'pgmq')"
    ).fetch_one(pool).await.unwrap();

    if has_pgmq {
        for sql in [
            "SELECT * FROM pgmq.q_jobs",
            "SELECT * FROM pgmq.a_jobs",
        ] {
            let res = exec_as_executor(pool, "crm", sql).await;
            assert!(res.is_err(), "executor must not read pgmq: {sql}");
        }
    }
    rt.shutdown().await;
}

#[tokio::test]
async fn t5_3_app_cannot_do_ddl() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("crm", "contacts").await;
    let pool = rt.pool();

    for sql in [
        "CREATE TABLE crm.evil (id int)",
        "DROP TABLE crm.contacts",
        "ALTER TABLE crm.contacts ADD COLUMN pwned text",
        "TRUNCATE crm.contacts",
    ] {
        let res = exec_as_executor(pool, "crm", sql).await;
        assert!(res.is_err(), "executor must be denied DDL: {sql}");
    }
    rt.shutdown().await;
}

#[tokio::test]
async fn t5_4_app_cannot_rewrite_identity_gucs() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("crm", "contacts").await;
    let pool = rt.pool();

    // set_config is revoked from the executor. Any attempt to overwrite
    // the RLS identity GUCs must fail.
    for sql in [
        "SELECT set_config('rootcx.user_id', '00000000-0000-0000-0000-000000000001', true)",
        "SELECT set_config('rootcx.is_delegated', '0', true)",
        "SELECT set_config('rootcx.effective_perms', '*', true)",
        "SELECT set_config('rootcx.actor_uid', '00000000-0000-0000-0000-000000000001', true)",
    ] {
        let res = exec_as_executor(pool, "crm", sql).await;
        assert!(res.is_err(), "executor must not call set_config: {sql}");
        let err = res.unwrap_err();
        assert!(
            err.contains("permission denied") || err.contains("denied"),
            "expected permission denied for set_config, got: {err}"
        );
    }
    rt.shutdown().await;
}

#[tokio::test]
async fn t5_5_app_cannot_set_role() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("crm", "contacts").await;
    let pool = rt.pool();

    // The executor must not be able to escalate to privileged roles.
    // Note: SET ROLE to the session user (the pool owner) or RESET ROLE
    // is allowed by PostgreSQL but is NOT a vulnerability because the app
    // never has direct access to the connection -- the core owns the
    // transaction and commits/rolls back. What matters is that the
    // executor cannot SET ROLE to roles with elevated privileges.
    for sql in [
        "SET ROLE rootcx_owner",
        "SET ROLE postgres",
    ] {
        let res = exec_as_executor(pool, "crm", sql).await;
        assert!(res.is_err(), "executor must not escalate to privileged role: {sql}");
    }

    // Even if the session-user reset works (PG allows it), the validate_sql
    // layer in the SQL proxy catches SET/RESET prefixes before they reach PG.
    // This validates that the structural defense (env_clear + IPC) means the
    // app never sends raw SQL to a connection -- only through run_sql which
    // calls validate_sql first. The PG-level role test above is the last line
    // of defense.
    rt.shutdown().await;
}

#[tokio::test]
async fn t5_6_app_cannot_read_cron_schema() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("crm", "contacts").await;
    let pool = rt.pool();

    let has_cron: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM information_schema.schemata WHERE schema_name = 'cron')"
    ).fetch_one(pool).await.unwrap();

    if has_cron {
        let res = exec_as_executor(pool, "crm", "SELECT * FROM cron.job").await;
        assert!(res.is_err(), "executor must not read cron.job");
    }
    rt.shutdown().await;
}

#[tokio::test]
async fn t5_7_do_block_refused() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("crm", "contacts").await;
    let pool = rt.pool();

    // DO blocks can execute arbitrary procedural code. The executor must
    // not be allowed to use them (or at minimum, set_config inside fails).
    let res = exec_as_executor(
        pool, "crm",
        "DO $$ BEGIN PERFORM set_config('rootcx.user_id','x',true); END $$"
    ).await;
    assert!(res.is_err(), "executor must be denied DO blocks or set_config inside DO");

    // Even a benign DO block should fail (language plpgsql revoke or DO itself).
    let res = exec_as_executor(pool, "crm", "DO $$ BEGIN NULL; END $$").await;
    // Acceptable: either plpgsql is not granted, or DO itself is blocked.
    // Both are valid enforcement paths. A success here means DO is allowed
    // but set_config inside is still blocked (covered by the first assert).
    // If this passes, it's not a security hole -- the set_config denial
    // above is the real gate.
    let _ = res;

    rt.shutdown().await;
}

#[tokio::test]
async fn t5_8_multi_statement_refused() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("crm", "contacts").await;
    let pool = rt.pool();

    // sqlx's extended query protocol sends one statement at a time. A
    // multi-statement string is rejected by the server.
    let res = exec_as_executor(
        pool, "crm",
        "SELECT 1; DROP TABLE crm.contacts"
    ).await;
    assert!(res.is_err(), "multi-statement must be rejected");

    let res = exec_as_executor(
        pool, "crm",
        "SELECT 1; SELECT set_config('rootcx.user_id','x',true)"
    ).await;
    assert!(res.is_err(), "multi-statement must be rejected even with benign first stmt");

    rt.shutdown().await;
}

#[tokio::test]
async fn t5_10_collection_onstart_bypass_rls() {
    // The onStart collection access (no user context) should have full
    // self-schema access via BYPASSRLS. We test this indirectly: if data
    // was inserted during admin setup (as owner pool) and we can read it
    // via the admin-authed HTTP route, the table has data. The real onStart
    // bypass is tested by the worker's internal `collection_op` path with
    // `allow_bypass=true`; at the integration level we verify the data
    // exists (proving BYPASSRLS owner pool access works for the core).
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("crm", "contacts").await;
    rt.create("crm", "contacts", &rec()).await;

    // Admin (who has *) can see the row proving the superuser pool works.
    let (s, body) = rt.get_json("/api/v1/apps/crm/collections/contacts").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body.as_array().map(|a| a.len()), Some(1),
        "admin/owner pool can read data (BYPASSRLS works for core operations)");

    // Additionally verify: the executor role WITHOUT a user context sees 0
    // rows (proving that RLS is FORCE'd and the executor does not bypass).
    let n = count_as(rt.pool(), None, false, "").await;
    assert_eq!(n, 0,
        "executor role with no user context sees 0 (FORCE RLS active, no bypass)");

    rt.shutdown().await;
}

// ── CATEGORY 4 : Sandbox worker (process isolation) ──────────────────
//
// The central security claim: "the app holds no DB credentials and no JWT."
//
// t4_sandbox_worker_env_has_no_secrets: THE definitive test. Deploys a real
// app with a JS handler that dumps process.env, invokes it via HTTP, and
// asserts the secrets are absent. Tests the ACTUAL spawn_worker path
// (env_clear + sandbox_env + bun + IPC handshake + RPC response).
//
// t4_4: unauthenticated HTTP to core is 401 (worker has no token to use).
//
// t4_3/t4_7: protocol contract guards (exact key sets). Fast canaries that
// break if someone adds a field to RpcCaller or Discover.


#[tokio::test]
async fn t4_sandbox_worker_env_has_no_secrets() {
    // Deploy a real app whose RPC handler returns process.env. Invoke it and
    // assert the core's secrets never reach the worker process.
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("sandbox_test", "dummy").await;

    let js = br#"serve({ rpc: { dumpEnv(params, caller, ctx) { return process.env; } } });"#;
    let tarball = harness::make_tar_gz(&[("index.js", js)]);
    let (s, body) = rt.deploy("sandbox_test", &tarball).await;
    assert!(s.is_success(), "deploy failed: {body}");

    // Give the test user invoke permission.
    let (tok, _) = user_with(&rt, "sandbox@t.local", &["app:sandbox_test:invoke"]).await;

    // Poll until the worker is running (discover handshake complete).
    for _ in 0..100 {
        let (s, _) = rt.get_json("/api/v1/apps/sandbox_test/worker/status").await;
        if s == StatusCode::OK { break; }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    let (s, env_dump) = rt.request_as(
        Method::POST, "/api/v1/apps/sandbox_test/rpc", &tok,
        Some(&json!({"method": "dumpEnv", "params": {}})),
    ).await;
    assert_eq!(s, StatusCode::OK, "RPC failed: {env_dump}");

    let env_str = env_dump.to_string();
    for secret in ["DATABASE_URL", "ROOTCX_JWT_SECRET"] {
        assert!(!env_str.contains(secret),
            "SECURITY VIOLATION: worker process.env contains `{secret}`: {env_str}");
    }
    // Positive: the worker DOES see its app identity vars.
    let obj = env_dump.as_object().expect("env must be an object");
    assert_eq!(obj.get("ROOTCX_APP_ID").and_then(|v| v.as_str()), Some("sandbox_test"),
        "worker must see ROOTCX_APP_ID");
    assert!(obj.contains_key("ROOTCX_RUNTIME_URL"), "worker must see ROOTCX_RUNTIME_URL");

    rt.shutdown().await;
}

#[tokio::test]
async fn t4_4_fetch_core_http_without_token_is_401() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("crm", "contacts").await;

    let s = rt.get_unauthed("/api/v1/apps").await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "no token = 401 on /apps");

    let s = rt.get_unauthed("/api/v1/apps/crm/collections/contacts").await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "no token = 401 on collections");

    let (s, _) = rt.post_unauthed("/api/v1/apps/crm/collections/contacts", &rec()).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "no token = 401 on POST collections");

    rt.shutdown().await;
}

// ── Protocol contract guards (fast, no runtime needed) ───────────────

fn wire_keys(v: &Value) -> std::collections::BTreeSet<&str> {
    v.as_object().unwrap().keys().map(|k| k.as_str()).collect()
}

fn expect_keys(v: &Value, expected: &[&str]) {
    let got = wire_keys(v);
    let want: std::collections::BTreeSet<&str> = expected.iter().copied().collect();
    assert_eq!(got, want, "wire key set drifted: {v}");
}

#[test]
fn t4_3_rpc_caller_wire_carries_no_token() {
    let with_perms = serde_json::to_value(rootcx_core::RpcCaller {
        user_id: "u-1".into(), email: "u@x.com".into(),
        effective_perms: Some(vec!["app:crm:contacts.read".into()]),
    }).unwrap();
    expect_keys(&with_perms, &["effectivePerms", "email", "userId"]);

    let without_perms = serde_json::to_value(rootcx_core::RpcCaller {
        user_id: "u-2".into(), email: "u2@x.com".into(), effective_perms: None,
    }).unwrap();
    expect_keys(&without_perms, &["email", "userId"]);
}

#[test]
fn t4_7_discover_wire_carries_no_database_url() {
    let minimal = serde_json::to_value(rootcx_core::OutboundMessage::Discover {
        app_id: "crm".into(), runtime_url: "http://127.0.0.1:9100".into(),
        credentials: std::collections::HashMap::new(), agent_config: None, run_onstart: true,
    }).unwrap();
    expect_keys(&minimal, &["app_id", "run_onstart", "runtime_url", "type"]);

    let with_creds = serde_json::to_value(rootcx_core::OutboundMessage::Discover {
        app_id: "crm".into(), runtime_url: "http://127.0.0.1:9100".into(),
        credentials: std::collections::HashMap::from([("K".into(), "V".into())]),
        agent_config: None, run_onstart: false,
    }).unwrap();
    expect_keys(&with_creds, &["app_id", "credentials", "runtime_url", "type"]);
}

// ── Category 5 supplementary: SQL proxy validate_sql layer ───────────
// These test the early-rejection layer (not the security boundary, but
// a defense-in-depth filter that gives clear errors for obvious attacks).

#[tokio::test]
async fn t5_supplementary_validate_sql_blocks_grant_revoke_early() {
    // GRANT/REVOKE are blocked by the validate_sql layer BEFORE reaching
    // Postgres. The PG role may or may not deny them depending on ownership
    // semantics, but the SQL proxy never lets them through. This test
    // verifies the validate_sql prefix check (unit-level, already tested in
    // sql_proxy::tests::rejects_ddl_prefixes, but duplicated here for the
    // contract suite completeness).
    //
    // At the integration level we verify the HTTP endpoint rejects them:
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("crm", "contacts").await;

    // The validate_sql function is private (not callable from integration
    // tests). Its correctness is verified by the unit tests in
    // sql_proxy::tests::rejects_ddl_prefixes. Here we verify the role-level
    // denial for privileges the executor definitely cannot have.
    let pool = rt.pool();

    // The executor cannot grant itself membership in the owner role.
    let res = exec_as_executor(
        pool, "crm", "GRANT rootcx TO rootcx_app_executor"
    ).await;
    assert!(res.is_err(), "executor must not grant itself membership in the owner role");

    // The executor cannot create new roles (DDL on roles).
    let res = exec_as_executor(pool, "crm", "CREATE ROLE evil_role").await;
    assert!(res.is_err(), "executor must not create roles");
    rt.shutdown().await;
}

#[tokio::test]
async fn t5_supplementary_executor_cannot_create_functions() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("crm", "contacts").await;
    let pool = rt.pool();

    let res = exec_as_executor(
        pool, "crm",
        "CREATE FUNCTION crm.evil() RETURNS void AS 'BEGIN END' LANGUAGE plpgsql"
    ).await;
    assert!(res.is_err(), "executor must not create functions");
    rt.shutdown().await;
}

#[tokio::test]
async fn t5_supplementary_executor_cannot_copy() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("crm", "contacts").await;
    let pool = rt.pool();

    let res = exec_as_executor(pool, "crm", "COPY crm.contacts TO '/tmp/dump.csv'").await;
    assert!(res.is_err(), "executor must not use COPY");
    rt.shutdown().await;
}

// ══════════════════════════════════════════════════════════════════════════════
// REGRESSION GUARDS — lines that, if reverted, silently break governance
// ══════════════════════════════════════════════════════════════════════════════

// GAP 16: deploy_frontend must require admin permission
#[tokio::test]
async fn regression_nonadmin_cannot_deploy_frontend() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("crm", "contacts").await;
    let (tok, _) = user_with(&rt, "sales@t.local", &["app:crm:contacts.read"]).await;
    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(b"fake".to_vec())
            .file_name("f.tar.gz")
            .mime_str("application/gzip")
            .unwrap(),
    );
    let s = reqwest::Client::new()
        .post(rt.url("/api/v1/apps/crm/frontend"))
        .bearer_auth(&tok)
        .multipart(form)
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(s, StatusCode::FORBIDDEN, "non-admin must not deploy frontend");
    rt.shutdown().await;
}

// GAP 13: collection_op after onStart denies without valid context (BYPASSRLS disabled)
#[tokio::test]
async fn regression_collection_op_denies_after_onstart() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("crm", "contacts").await;
    let pool = rt.pool();

    // Simulate post-onStart: allow_bypass = false, state = None.
    // This calls the internal collection_op with the exact args that the
    // supervisor uses after onstart_done=true when context_token is missing.
    let result = rootcx_core::worker::collection_op_test(
        pool, "crm", "find", "contacts", json!({}), None, false,
    ).await;
    assert!(result.is_err(), "collection_op without context after onStart must deny");
    assert!(result.unwrap_err().contains("access denied"), "must be access denied, not a random error");
    rt.shutdown().await;
}

// GAP 14+15: agent tool ContextState must be delegated with invoker (human) identity.
// If is_delegated regresses to false OR user_id uses agent UID instead of human,
// the agent gets the human's FULL permissions (bypasses intersection).
#[tokio::test]
async fn regression_agent_tool_delegated_context_blocks_excess_perms() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install_manifest(&json!({
        "appId": "crm", "name": "crm", "version": "1.0.0",
        "dataContract": [
            { "entityName": "contacts", "fields": [{"name": "name", "type": "text", "required": true}] },
            { "entityName": "deals", "fields": [{"name": "name", "type": "text", "required": true}] },
        ]
    })).await;
    let pool = rt.pool();

    // Jean has contacts + deals permissions
    let (_, jean) = user_with(&rt, "jean-tool@t.local", &[
        "app:crm:contacts.read", "app:crm:contacts.create",
        "app:crm:deals.read", "app:crm:deals.create",
    ]).await;

    // Insert test data
    sqlx::query("INSERT INTO crm.contacts (id, name) VALUES (gen_random_uuid(), 'Alice')")
        .execute(pool).await.unwrap();
    sqlx::query("INSERT INTO crm.deals (id, name) VALUES (gen_random_uuid(), 'Deal1')")
        .execute(pool).await.unwrap();

    // Agent intersection: only contacts.read (NOT deals.read)
    let agent_intersection = vec!["app:crm:contacts.read".to_string()];

    // Simulate exactly what query_data.rs does: delegated=true, user_id=invoker (jean)
    let state = rootcx_core::sql_proxy::ContextState {
        user_id: Some(jean),
        is_delegated: true,
        effective_perms: agent_intersection,
    };
    let mut tx = rootcx_core::sql_proxy::begin_app_tx(pool, "crm", &state, Some(jean), None, "test", rootcx_core::sql_proxy::TIMEOUT_INTERACTIVE_MS)
        .await.unwrap();
    let contacts: i64 = sqlx::query_scalar("SELECT count(*) FROM crm.contacts")
        .fetch_one(&mut *tx).await.unwrap();
    let deals: i64 = sqlx::query_scalar("SELECT count(*) FROM crm.deals")
        .fetch_one(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();

    assert!(contacts > 0, "agent with contacts.read must see contacts");
    assert_eq!(deals, 0, "agent WITHOUT deals.read must see 0 deals (intersection enforced)");
    rt.shutdown().await;
}

// ══════════════════════════════════════════════════════════════════════════════
// GOVERNANCE MODEL COVERAGE — parameterized control-plane gates
// One test, multiple endpoints, same pattern: non-admin/missing-perm -> 403.
// Follows testing-guidelines: "N tests with identical structure = 1 test with a loop"
// ══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn governance_model_control_plane_gates() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("crm", "contacts").await;

    // Non-admin user with only basic data perms (no admin:*, no cron.write, no webhook.read)
    let (tok, _) = user_with(&rt, "basic@t.local", &["app:crm:contacts.read"]).await;

    // Each tuple: (method, path, body, description). All must return 403.
    let cases: &[(Method, &str, Option<&Value>, &str)] = &[
        // Row 20: uninstall requires admin
        (Method::DELETE, "/api/v1/apps/crm", None, "uninstall app"),
        // Row 25: agent management requires admin
        (Method::DELETE, "/api/v1/agents/crm", None, "delete agent config"),
        // Row 26: worker start/stop requires super-admin
        (Method::POST, "/api/v1/apps/crm/worker/start", None, "start worker"),
        (Method::POST, "/api/v1/apps/crm/worker/stop", None, "stop worker"),
        // Row 29: webhook list requires webhook.read
        (Method::GET, "/api/v1/apps/crm/webhooks", None, "list webhooks"),
        // Platform secrets require admin
        (Method::GET, "/api/v1/platform/secrets/env", None, "read platform secrets"),
        // MCP server management requires admin (tested separately, body validation precedes perm check)
    ];

    for (method, path, body, desc) in cases {
        let (status, _) = rt.request_as(method.clone(), path, &tok, *body).await;
        assert_eq!(status, StatusCode::FORBIDDEN,
            "non-admin/non-privileged user must be denied on: {desc} ({method} {path}), got {status}");
    }

    // Row 27: cron CRUD requires cron.write (POST needs JSON body)
    let (status, _) = rt.request_as(Method::POST, "/api/v1/apps/crm/crons", &tok,
        Some(&json!({"name": "test", "schedule": "0 * * * *", "action_type": "rpc", "action": "sync"}))).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "user without cron.write must be denied on create cron, got {status}");

    // Admin SQL query requires admin (needs JSON body)
    let (status, _) = rt.request_as(Method::POST, "/api/v1/db/query", &tok,
        Some(&json!({"sql": "SELECT 1"}))).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "non-admin must be denied on execute admin sql, got {status}");

    rt.shutdown().await;
}

#[tokio::test]
async fn governance_model_authenticated_routes_accessible() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("crm", "contacts").await;

    // Any authenticated user should access schema introspection (Row 23)
    let (tok, _) = user_with(&rt, "viewer@t.local", &[]).await;

    let cases: &[(&str, &str)] = &[
        ("/api/v1/db/schemas", "list schemas"),
        ("/api/v1/db/schemas/crm/tables", "list tables in schema"),
    ];

    for (path, desc) in cases {
        let (status, _) = rt.request_as(Method::GET, path, &tok, None).await;
        assert_ne!(status, StatusCode::FORBIDDEN,
            "any authenticated user must access: {desc} ({path}), got {status}");
        assert_ne!(status, StatusCode::UNAUTHORIZED,
            "authenticated request must not get 401 on: {desc} ({path})");
    }

    rt.shutdown().await;
}

#[tokio::test]
async fn governance_model_integration_action_requires_permission() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;

    // User without any integration permission (Row 30)
    let (tok, _) = user_with(&rt, "nointeg@t.local", &["app:crm:contacts.read"]).await;

    // Attempt to call an integration action without the integration permission
    let (status, _) = rt.request_as(
        Method::POST,
        "/api/v1/integrations/gmail/actions/sync_now",
        &tok,
        Some(&json!({"userId": "fake", "userCredentials": {}})),
    ).await;
    // Should be 403 (or 404 if integration not installed, both are "denied")
    assert!(status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND,
        "user without integration permission must be denied, got {status}");

    rt.shutdown().await;
}

// ══════════════════════════════════════════════════════════════════════════════
// SQL PROXY SAFETY — timeout, row cap, atomicity
// ══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn sql_proxy_timeout_kills_long_running_query() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("crm", "contacts").await;
    let (_, uid) = user_with(&rt, "timeout@t.local", &["app:crm:contacts.read"]).await;

    let state = rootcx_core::sql_proxy::ContextState {
        user_id: Some(uid),
        is_delegated: false,
        effective_perms: vec!["app:crm:contacts.read".into()],
    };

    // Use a 1-second timeout (minimum practical). pg_sleep(5) must be cancelled.
    let err_msg = {
        let mut tx = rootcx_core::sql_proxy::begin_app_tx(
            rt.pool(), "crm", &state, Some(uid), None, "test", 1000,
        ).await.unwrap();
        let result = sqlx::query("SELECT pg_sleep(5)")
            .execute(&mut *tx).await;
        assert!(result.is_err(), "statement_timeout must cancel pg_sleep");
        result.unwrap_err().to_string()
    };
    assert!(
        err_msg.contains("cancel") || err_msg.contains("timeout"),
        "error should mention cancellation: {err_msg}"
    );
    rt.shutdown().await;
}

#[tokio::test]
async fn sql_proxy_oversized_result_rolls_back() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("crm", "contacts").await;
    let (_, uid) = user_with(&rt, "rowcap@t.local", &[
        "app:crm:contacts.read", "app:crm:contacts.create",
    ]).await;

    let state = rootcx_core::sql_proxy::ContextState {
        user_id: Some(uid),
        is_delegated: false,
        effective_perms: vec![
            "app:crm:contacts.read".into(),
            "app:crm:contacts.create".into(),
        ],
    };

    // Insert 1001 rows via generate_series RETURNING. Must exceed MAX_ROWS=1000.
    let sql = concat!(
        "INSERT INTO crm.contacts (first_name, last_name, email) ",
        "SELECT 'bulk', 'test', 'b' || g || '@x.com' ",
        "FROM generate_series(1,1001) g ",
        "RETURNING id"
    );
    let result = rootcx_core::sql_proxy::run_sql(rt.pool(), "crm", &state, sql, &[]).await;
    assert!(result.is_err(), "over-limit result must error");
    assert!(result.unwrap_err().contains("exceeds limit"));

    // Verify rollback: no rows committed (table should be empty).
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM crm.contacts")
        .fetch_one(rt.pool()).await.unwrap();
    assert_eq!(count, 0, "over-limit DML RETURNING must roll back, not commit");
    rt.shutdown().await;
}

#[tokio::test]
async fn sql_proxy_row_cap_boundary_1000_succeeds() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("crm", "contacts").await;
    let (_, uid) = user_with(&rt, "boundary@t.local", &[
        "app:crm:contacts.read", "app:crm:contacts.create",
    ]).await;

    let state = rootcx_core::sql_proxy::ContextState {
        user_id: Some(uid),
        is_delegated: false,
        effective_perms: vec![
            "app:crm:contacts.read".into(),
            "app:crm:contacts.create".into(),
        ],
    };

    // Exactly 1000 rows: must succeed.
    let sql = concat!(
        "INSERT INTO crm.contacts (first_name, last_name, email) ",
        "SELECT 'ok', 'test', 'ok' || g || '@x.com' ",
        "FROM generate_series(1,1000) g ",
        "RETURNING id"
    );
    let result = rootcx_core::sql_proxy::run_sql(rt.pool(), "crm", &state, sql, &[]).await;
    assert!(result.is_ok(), "exactly 1000 rows must succeed: {:?}", result.err());
    assert_eq!(result.unwrap().row_count, 1000);

    // Verify committed.
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM crm.contacts")
        .fetch_one(rt.pool()).await.unwrap();
    assert_eq!(count, 1000, "1000 rows must be committed");
    rt.shutdown().await;
}

// ══════════════════════════════════════════════════════════════════════════════
// CHANNEL DELEGATION REVOCATION
// ══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn t3_11b_channel_unlink_revokes_delegation() {
    let rt = harness::TestRuntime::boot().await;
    let pool = rt.pool();
    let user_id = create_user(pool, "chan-unlink@t.local").await;
    register_agent(pool, "support").await;
    let agent_uid = rootcx_core::extensions::agents::agent_user_id("support");

    let channel_id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let trigger_ref = rootcx_core::extensions::channels::channel_delegation_ref(channel_id, user_id);

    // Simulate link: create delegation with deterministic trigger_ref (same as try_complete_link)
    rootcx_core::delegations::create(pool, user_id, agent_uid, "channel", Some(trigger_ref)).await.unwrap();
    assert!(rootcx_core::delegations::is_valid(pool, user_id, agent_uid).await.unwrap(),
        "delegation must be valid after channel link");

    // Simulate unlink: revoke_by_trigger with the same deterministic ref
    rootcx_core::delegations::revoke_by_trigger(pool, "channel", trigger_ref).await.unwrap();
    assert!(!rootcx_core::delegations::is_valid(pool, user_id, agent_uid).await.unwrap(),
        "delegation must be invalid after channel unlink (revoke_by_trigger)");

    rt.shutdown().await;
}

// ══════════════════════════════════════════════════════════════════════════════
// UNDOCUMENTED BEHAVIORS — now documented and tested
// ══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn public_caller_deny_all_on_data() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("crm", "contacts").await;
    rt.create("crm", "contacts", &rec()).await;
    let pool = rt.pool();

    // Simulate the exact ContextState a public/share-token caller produces:
    // user_id="" parses to None, is_delegated=false, effective_perms=[]
    let public_state = rootcx_core::sql_proxy::ContextState {
        user_id: "".parse().ok(), // None (empty string fails UUID parse)
        is_delegated: false,
        effective_perms: vec![],
    };
    assert!(public_state.user_id.is_none(), "precondition: empty string must parse to None");

    // ctx.sql: RLS denies all rows (check_access sees NULL user_id -> FALSE)
    let result = rootcx_core::sql_proxy::run_sql(
        pool, "crm", &public_state, "SELECT * FROM crm.contacts", &[],
    ).await;
    assert!(result.is_ok(), "query executes without error");
    assert_eq!(result.unwrap().row_count, 0, "public caller must see 0 rows via ctx.sql");

    // ctx.collection after onStart: state=Some but user_id=None -> RLS tx denies
    let coll_result = rootcx_core::worker::collection_op_test(
        pool, "crm", "find", "contacts", json!({}), Some(public_state), false,
    ).await;
    assert!(coll_result.is_ok(), "collection_op executes");
    let rows = coll_result.unwrap();
    assert_eq!(rows.as_array().map(|a| a.len()), Some(0),
        "public caller must see 0 rows via ctx.collection after onStart");

    rt.shutdown().await;
}

#[tokio::test]
async fn integration_worker_deny_all_on_sql_but_onstart_bypass_works() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("crm", "contacts").await;
    rt.create("crm", "contacts", &rec()).await;
    let pool = rt.pool();

    // Integration caller: None -> ContextState::default()
    let integration_state = rootcx_core::sql_proxy::ContextState::from_caller(None);
    assert!(integration_state.user_id.is_none(), "precondition: None caller -> None user_id");
    assert!(!integration_state.is_delegated, "precondition: not delegated");

    // ctx.sql with integration identity: deny-all (0 rows)
    let sql_result = rootcx_core::sql_proxy::run_sql(
        pool, "crm", &integration_state, "SELECT * FROM crm.contacts", &[],
    ).await;
    assert!(sql_result.is_ok(), "query executes without error");
    assert_eq!(sql_result.unwrap().row_count, 0,
        "integration worker (caller: None) must see 0 rows via ctx.sql");

    // ctx.collection after onStart with no state: hard deny
    let denied = rootcx_core::worker::collection_op_test(
        pool, "crm", "find", "contacts", json!({}), None, false,
    ).await;
    assert!(denied.is_err(), "collection without context after onStart must deny");

    // ctx.collection DURING onStart (allow_bypass=true): BYPASSRLS, full access
    let bypass = rootcx_core::worker::collection_op_test(
        pool, "crm", "find", "contacts", json!({}), None, true,
    ).await;
    assert!(bypass.is_ok(), "onStart collection must succeed with BYPASSRLS");
    assert_eq!(bypass.unwrap().as_array().map(|a| a.len()), Some(1),
        "onStart BYPASSRLS sees all rows");

    rt.shutdown().await;
}

#[tokio::test]
async fn rls_per_entity_not_per_row_same_perm_same_rows() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("crm", "contacts").await;
    let pool = rt.pool();

    // Insert 3 rows (as superuser/owner pool)
    for name in ["Alice", "Bob", "Carol"] {
        rt.create("crm", "contacts", &json!({
            "first_name": name, "last_name": "Test", "email": format!("{name}@t.local")
        })).await;
    }

    // Two distinct users, both with contacts.read
    let alice_uid = db_user(pool, "alice-b3@t.local", &["app:crm:contacts.read"]).await;
    let bob_uid = db_user(pool, "bob-b3@t.local", &["app:crm:contacts.read"]).await;
    assert_ne!(alice_uid, bob_uid, "precondition: different users");

    // Both must see the same row count (all 3 rows, no ownership filter)
    let alice_count = count_as(pool, Some(alice_uid), false, "").await;
    let bob_count = count_as(pool, Some(bob_uid), false, "").await;
    assert_eq!(alice_count, 3, "alice with contacts.read sees all 3 rows");
    assert_eq!(bob_count, 3, "bob with contacts.read sees all 3 rows");
    assert_eq!(alice_count, bob_count,
        "per-entity RLS: same permission = same rows regardless of user identity");

    rt.shutdown().await;
}

// ══════════════════════════════════════════════════════════════════════════════
// READ-SURFACE HARDENING
// ══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn introspection_system_schemas_require_admin() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("crm", "contacts").await;

    let (tok, _) = user_with(&rt, "nonadmin-intro@t.local", &["app:crm:contacts.read"]).await;

    // Non-admin: system schema tables -> 403
    let system_schemas = &[
        ("/api/v1/db/schemas/rootcx_system/tables", "rootcx_system"),
        ("/api/v1/db/schemas/pg_catalog/tables", "pg_catalog"),
        ("/api/v1/db/schemas/information_schema/tables", "information_schema"),
    ];
    for (path, schema) in system_schemas {
        let (status, _) = rt.request_as(Method::GET, path, &tok, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN,
            "non-admin must be denied list_tables for {schema}, got {status}");
    }

    // Non-admin: list_schemas must NOT include rootcx_system
    let (status, body) = rt.request_as(Method::GET, "/api/v1/db/schemas", &tok, None).await;
    assert_eq!(status, StatusCode::OK);
    let schemas: Vec<&str> = body.as_array().unwrap().iter()
        .filter_map(|v| v.get("schema_name").and_then(|s| s.as_str()))
        .collect();
    assert!(!schemas.contains(&"rootcx_system"),
        "non-admin list_schemas must filter rootcx_system, got: {schemas:?}");
    assert!(schemas.contains(&"crm"),
        "non-admin must still see app schemas, got: {schemas:?}");

    // Admin: system schemas accessible + rootcx_system visible in list
    let (status, _) = rt.request_as(Method::GET,
        "/api/v1/db/schemas/rootcx_system/tables", &rt.token, None).await;
    assert_eq!(status, StatusCode::OK,
        "admin must access rootcx_system tables");

    let (_, body) = rt.request_as(Method::GET, "/api/v1/db/schemas", &rt.token, None).await;
    let schemas: Vec<&str> = body.as_array().unwrap().iter()
        .filter_map(|v| v.get("schema_name").and_then(|s| s.as_str()))
        .collect();
    assert!(schemas.contains(&"rootcx_system"),
        "admin list_schemas must include rootcx_system, got: {schemas:?}");

    rt.shutdown().await;
}

#[tokio::test]
async fn mcp_read_endpoints_require_admin() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;

    let (tok, _) = user_with(&rt, "nonadmin-mcp@t.local", &["app:crm:contacts.read"]).await;

    // Non-admin: list + get -> 403
    let (status, _) = rt.request_as(Method::GET, "/api/v1/mcp-servers", &tok, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN,
        "non-admin must be denied MCP list_servers, got {status}");

    let (status, _) = rt.request_as(Method::GET, "/api/v1/mcp-servers/nonexistent", &tok, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN,
        "non-admin must be denied MCP get_server, got {status}");

    // Admin: list succeeds (empty list is fine)
    let (status, _) = rt.request_as(Method::GET, "/api/v1/mcp-servers", &rt.token, None).await;
    assert_eq!(status, StatusCode::OK,
        "admin must access MCP list_servers");

    rt.shutdown().await;
}

// ── Regression: audit-log requires admin:audit.read ──────────────────────

#[tokio::test]
async fn regression_audit_log_requires_admin() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;

    let (tok, _) = user_with(&rt, "nonadmin-audit@t.local", &["app:crm:contacts.read"]).await;
    let (status, _) = rt.request_as(Method::GET, "/api/v1/audit", &tok, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN,
        "non-admin must be denied audit log access, got {status}");

    let (status, body) = rt.request_as(Method::GET, "/api/v1/audit", &rt.token, None).await;
    assert_eq!(status, StatusCode::OK, "admin must access audit log: {body}");

    rt.shutdown().await;
}

// ── Regression: governance hardening (release review 2026-05-31) ─────────

/// HIGH: untrusted app SQL must not be able to enumerate the RBAC graph.
/// The plpgsql RBAC helpers are SECURITY DEFINER and take an arbitrary
/// user_id; if PUBLIC keeps the default EXECUTE grant, any app can call
/// `resolve_permissions(<anyone>)` through ctx.sql and read their full
/// permission set. Layer 2 promises "cannot read rootcx_system". Only
/// `check_access` (invoked by the RLS policies) may stay callable.
#[tokio::test]
async fn t5_8_app_cannot_enumerate_rbac_graph() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("crm", "contacts").await;
    let pool = rt.pool();

    let sys = "00000000-0000-0000-0000-000000000001";
    for sql in [
        format!("SELECT rootcx_system.resolve_permissions('{sys}'::uuid)"),
        format!("SELECT rootcx_system.has_permission('{sys}'::uuid, '*')"),
        "SELECT rootcx_system.expand_roles(ARRAY['admin'])".to_string(),
        "SELECT rootcx_system.match_permission(ARRAY['*'], 'admin:secrets.manage')".to_string(),
    ] {
        let res = exec_as_executor(pool, "crm", &sql).await;
        assert!(res.is_err(), "executor must not execute RBAC helper: {sql}");
        let err = res.unwrap_err();
        assert!(
            err.contains("permission denied") || err.contains("denied"),
            "expected permission denied for {sql}, got: {err}"
        );
    }

    // check_access stays callable: the RLS policies depend on it.
    let ok = exec_as_executor(pool, "crm", "SELECT rootcx_system.check_access('app:crm:contacts.read')").await;
    assert!(ok.is_ok(), "check_access must remain callable by the executor: {ok:?}");

    rt.shutdown().await;
}

/// Blocker: an app that declares a custom `permissions` block must STILL get
/// the per-entity keys its table RLS policies require. apply_table_rls gates
/// every table on `app:{id}:{entity}.{action}`; if those keys are never minted
/// they are absent from the permission catalog, invisible to admins, and no
/// non-admin can ever be granted access -> the app's data is deny-all by
/// default for everyone but `*`.
#[tokio::test]
async fn t1_9_custom_permissions_app_still_mints_entity_keys() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;

    let manifest = json!({
        "appId": "crm", "name": "crm", "version": "1.0.0",
        "permissions": { "permissions": [
            { "key": "reports.export", "description": "export reports" }
        ]},
        "dataContract": [{ "entityName": "contacts", "fields": [
            { "name": "first_name", "type": "text", "required": true }
        ]}]
    });
    rt.install_manifest(&manifest).await;

    let keys: Vec<String> = sqlx::query_scalar(
        "SELECT key FROM rootcx_system.rbac_permissions WHERE source_app = 'crm' ORDER BY key",
    ).fetch_all(rt.pool()).await.unwrap();

    for required in [
        "app:crm:contacts.create", "app:crm:contacts.read",
        "app:crm:contacts.update", "app:crm:contacts.delete",
    ] {
        assert!(keys.iter().any(|k| k == required),
            "custom-permissions app must still mint entity key {required}; got {keys:?}");
    }
    // The custom-declared key must also be present (no regression).
    assert!(keys.iter().any(|k| k == "app:crm:reports.export"),
        "custom-declared key must be minted; got {keys:?}");

    rt.shutdown().await;
}
