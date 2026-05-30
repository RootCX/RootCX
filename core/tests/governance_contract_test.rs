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
    assert_ne!(s, StatusCode::CREATED, "create without .create must be refused by RLS WITH CHECK");
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
async fn t6_3_nonadmin_cannot_run_db_query() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    let (tok, _) = user_with(&rt, "sales@t.local", &[]).await;
    let (s, _) = rt.request_as(Method::POST, "/api/v1/db/query", &tok,
        Some(&json!({"sql": "SELECT 1"}))).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    rt.shutdown().await;
}

#[tokio::test]
async fn t6_2_nonadmin_cannot_read_platform_secrets() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    let (tok, _) = user_with(&rt, "sales@t.local", &[]).await;
    let (s, _) = rt.request_as(Method::GET, "/api/v1/platform/secrets/env", &tok, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    rt.shutdown().await;
}

#[tokio::test]
async fn t6_7_nonadmin_cannot_manage_mcp() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    let (tok, _) = user_with(&rt, "sales@t.local", &[]).await;
    let (s, _) = rt.request_as(Method::POST, "/api/v1/mcp-servers", &tok,
        Some(&json!({"name": "x", "transport": {"type": "stdio", "command": "true", "args": []}, "autoStart": false}))).await;
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

#[tokio::test]
async fn db_app_executor_role_is_locked_down() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("crm", "contacts").await;
    let pool = rt.pool();

    // The restricted role cannot read system tables, cannot rewrite identity
    // GUCs, and cannot do DDL. Multi-statement is rejected structurally by the
    // extended query protocol (the reason validate_sql needs no `;` parser).
    // Each must error.
    for sql in [
        "SELECT * FROM rootcx_system.users",
        "SELECT set_config('rootcx.is_delegated','0',true)",
        "CREATE TABLE crm.evil (id int)",
        "SELECT 1; DROP TABLE crm.contacts",
    ] {
        let mut tx = pool.begin().await.unwrap();
        sqlx::query("SET LOCAL search_path TO crm, public").execute(&mut *tx).await.unwrap();
        sqlx::query("SET LOCAL ROLE rootcx_app_executor").execute(&mut *tx).await.unwrap();
        let res = sqlx::query(sql).execute(&mut *tx).await;
        assert!(res.is_err(), "executor must be denied: {sql}");
        let _ = tx.rollback().await;
    }
    rt.shutdown().await;
}

#[tokio::test]
async fn db_rls_filters_by_guc_identity() {
    let rt = harness::TestRuntime::boot().await;
    admin(&rt).await;
    rt.install("crm", "contacts").await;
    rt.create("crm", "contacts", &rec()).await;
    let pool = rt.pool();

    let jean: Uuid = Uuid::new_v4();
    sqlx::query("INSERT INTO rootcx_system.users (id, email) VALUES ($1, 'jean-db@t.local')")
        .bind(jean).execute(pool).await.unwrap();
    let role = "role_jean_db";
    sqlx::query("INSERT INTO rootcx_system.rbac_roles (name, permissions) VALUES ($1, ARRAY['app:crm:contacts.read']) ON CONFLICT (name) DO NOTHING")
        .bind(role).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO rootcx_system.rbac_assignments (user_id, role) VALUES ($1, $2)")
        .bind(jean).bind(role).execute(pool).await.unwrap();

    // With Jean's identity posed → visible.
    let n = count_as(pool, Some(jean), false, "").await;
    assert_eq!(n, 1, "Jean with read perm sees the row");

    // No identity → deny all.
    let n = count_as(pool, None, false, "").await;
    assert_eq!(n, 0, "NULL user_id → 0 rows");

    // Delegated with empty intersection → deny all (sentinel).
    let n = count_as(pool, Some(jean), true, "").await;
    assert_eq!(n, 0, "is_delegated=1 + empty effective_perms → deny all");

    // Delegated with the right perm in the intersection → visible.
    let n = count_as(pool, Some(jean), true, "app:crm:contacts.read").await;
    assert_eq!(n, 1, "delegated with perm in intersection → visible");
    rt.shutdown().await;
}

// ── Cross-agent invoke gate (TEST 3.13) ───────────────────────────────

#[tokio::test]
async fn sub_agent_invoke_requires_invoke_perm() {
    let rt = harness::TestRuntime::boot().await;
    // Effective authority (grant∩human) lacks app:billing:invoke → the agent
    // must NOT be able to invoke the billing agent. The gate fires before any
    // dispatch or I/O.
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
