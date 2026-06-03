//! App-ownership contract tests: an employee who installs an app automatically
//! becomes its admin (holds `app:{id}:*`), and can manage crons, hooks, jobs,
//! data, agent, and grant access to colleagues — WITHOUT being a platform admin.
//!
//! These tests define the DESIRED behavior. They are expected to FAIL until the
//! auto-assignment is implemented.

mod harness;

use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use uuid::Uuid;

/// Register a non-admin human and return their token + uid.
async fn employee(rt: &harness::TestRuntime, email: &str) -> (String, Uuid) {
    let tok = rt.register_and_login(email).await;
    let uid: Uuid = sqlx::query_scalar("SELECT id FROM rootcx_system.users WHERE email = $1")
        .bind(email).fetch_one(rt.pool()).await.unwrap();
    // Explicitly strip any admin role to prove this user is NOT a platform admin.
    sqlx::query("DELETE FROM rootcx_system.rbac_assignments WHERE user_id = $1")
        .bind(uid).execute(rt.pool()).await.unwrap();
    (tok, uid)
}

/// Give the platform admin role to the harness user so platform-level setup works.
async fn ensure_admin(rt: &harness::TestRuntime) {
    sqlx::query(
        "INSERT INTO rootcx_system.rbac_assignments (user_id, role) \
         SELECT id, 'admin' FROM rootcx_system.users WHERE email = 'admin@test.local' \
         ON CONFLICT DO NOTHING"
    ).execute(rt.pool()).await.unwrap();
}

/// Give an employee a lightweight platform permission to install apps (not `*`).
async fn allow_install(rt: &harness::TestRuntime, uid: Uuid) {
    // Ensure the permission key and a role exist for it.
    sqlx::query("INSERT INTO rootcx_system.rbac_permissions (key, description) VALUES ('platform:apps.create', 'Create apps') ON CONFLICT DO NOTHING")
        .execute(rt.pool()).await.unwrap();
    sqlx::query("INSERT INTO rootcx_system.rbac_roles (name, permissions) VALUES ('app_creator', ARRAY['platform:apps.create']) ON CONFLICT (name) DO NOTHING")
        .execute(rt.pool()).await.unwrap();
    sqlx::query("INSERT INTO rootcx_system.rbac_assignments (user_id, role) VALUES ($1, 'app_creator') ON CONFLICT DO NOTHING")
        .bind(uid).execute(rt.pool()).await.unwrap();
}

fn manifest(app_id: &str) -> Value {
    json!({
        "appId": app_id, "name": app_id, "version": "1.0.0",
        "dataContract": [{ "entityName": "tasks", "fields": [
            { "name": "title", "type": "text", "required": true },
            { "name": "status", "type": "text" },
        ]}]
    })
}

// ── Core use case: employee installs app and becomes its admin ────────

#[tokio::test]
async fn employee_installs_app_and_gets_app_admin() {
    let rt = harness::TestRuntime::boot().await;
    ensure_admin(&rt).await;
    let (tok, uid) = employee(&rt, "dev@company.local").await;
    allow_install(&rt, uid).await;

    // Employee (non-admin) installs their app.
    let (s, body) = rt.request_as(Method::POST, "/api/v1/apps", &tok, Some(&manifest("myapp"))).await;
    assert_eq!(s, StatusCode::OK, "employee with platform:apps.create must be able to install: {body}");

    // After install, the employee holds app:myapp:* (auto-assigned).
    let (_, perms) = rt.request_as(Method::GET, "/api/v1/permissions", &tok, None).await;
    let perm_list = perms["permissions"].as_array().expect("permissions array");
    let has_wildcard = perm_list.iter().any(|p| p.as_str() == Some("app:myapp:*"));
    assert!(has_wildcard, "installer must auto-receive app:myapp:* — got {perms}");
    rt.shutdown().await;
}

// ── App owner manages sub-resources (parameterized) ──────────────────

#[tokio::test]
async fn app_owner_manages_sub_resources() {
    let rt = harness::TestRuntime::boot().await;
    ensure_admin(&rt).await;
    let (tok, uid) = employee(&rt, "dev2@company.local").await;
    allow_install(&rt, uid).await;
    rt.request_as(Method::POST, "/api/v1/apps", &tok, Some(&manifest("ownapp"))).await;

    let cases: &[(&str, Value, StatusCode)] = &[
        ("/api/v1/apps/ownapp/crons", json!({ "name": "nightly", "schedule": "0 2 * * *" }), StatusCode::CREATED),
        ("/api/v1/apps/ownapp/hooks", json!({ "entity": "tasks", "operation": "INSERT", "action_type": "job" }), StatusCode::OK),
        ("/api/v1/apps/ownapp/jobs", json!({ "payload": { "task": "sync" } }), StatusCode::CREATED),
    ];
    for (path, payload, expected) in cases {
        let (s, body) = rt.request_as(Method::POST, path, &tok, Some(payload)).await;
        assert_eq!(s, *expected, "{path} failed: {body}");
    }
    rt.shutdown().await;
}

// ── Data: employee reads/writes data in their own app ────────────────

#[tokio::test]
async fn app_owner_reads_and_writes_data() {
    let rt = harness::TestRuntime::boot().await;
    ensure_admin(&rt).await;
    let (tok, uid) = employee(&rt, "dev5@company.local").await;
    allow_install(&rt, uid).await;
    rt.request_as(Method::POST, "/api/v1/apps", &tok, Some(&manifest("dataapp"))).await;

    // Write
    let (s, body) = rt.request_as(
        Method::POST, "/api/v1/apps/dataapp/collections/tasks", &tok,
        Some(&json!({ "title": "Test task", "status": "open" })),
    ).await;
    assert_eq!(s, StatusCode::CREATED, "app owner must write data: {body}");

    // Read
    let (s, body) = rt.request_as(Method::GET, "/api/v1/apps/dataapp/collections/tasks", &tok, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body.as_array().map(|a| a.len()), Some(1), "app owner must see their data: {body}");
    rt.shutdown().await;
}

// ── Colleague: cannot access app without permission from owner ───────

#[tokio::test]
async fn colleague_without_permission_sees_nothing() {
    let rt = harness::TestRuntime::boot().await;
    ensure_admin(&rt).await;
    let (tok_owner, uid_owner) = employee(&rt, "owner@company.local").await;
    allow_install(&rt, uid_owner).await;
    rt.request_as(Method::POST, "/api/v1/apps", &tok_owner, Some(&manifest("private"))).await;

    // Owner writes data.
    rt.request_as(
        Method::POST, "/api/v1/apps/private/collections/tasks", &tok_owner,
        Some(&json!({ "title": "Secret", "status": "open" })),
    ).await;

    // Colleague (no permissions on this app) sees nothing.
    let (tok_colleague, _) = employee(&rt, "colleague@company.local").await;
    let (s, body) = rt.request_as(Method::GET, "/api/v1/apps/private/collections/tasks", &tok_colleague, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body.as_array().map(|a| a.len()), Some(0),
        "colleague without permission must see 0 rows (RLS deny)");

    // Colleague cannot invoke.
    let (s, _) = rt.request_as(
        Method::POST, "/api/v1/apps/private/jobs", &tok_colleague,
        Some(&json!({ "payload": {} })),
    ).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "colleague without invoke perm must be denied");
    rt.shutdown().await;
}

// ── Employee cannot touch another employee's app ─────────────────────

#[tokio::test]
async fn employee_cannot_manage_other_apps() {
    let rt = harness::TestRuntime::boot().await;
    ensure_admin(&rt).await;
    let (tok_a, uid_a) = employee(&rt, "alice@company.local").await;
    let (tok_b, uid_b) = employee(&rt, "bob@company.local").await;
    allow_install(&rt, uid_a).await;
    allow_install(&rt, uid_b).await;

    // Alice creates her app.
    rt.request_as(Method::POST, "/api/v1/apps", &tok_a, Some(&manifest("alice_app"))).await;
    // Bob creates his app.
    rt.request_as(Method::POST, "/api/v1/apps", &tok_b, Some(&manifest("bob_app"))).await;

    // Alice writes data in her app.
    rt.request_as(
        Method::POST, "/api/v1/apps/alice_app/collections/tasks", &tok_a,
        Some(&json!({ "title": "Alice task", "status": "open" })),
    ).await;

    // Bob cannot read Alice's data.
    let (s, body) = rt.request_as(Method::GET, "/api/v1/apps/alice_app/collections/tasks", &tok_b, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body.as_array().map(|a| a.len()), Some(0),
        "Bob must not see Alice's data");

    // Bob cannot create crons on Alice's app.
    let (s, _) = rt.request_as(
        Method::POST, "/api/v1/apps/alice_app/crons", &tok_b,
        Some(&json!({ "name": "evil", "schedule": "* * * * *" })),
    ).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "Bob must not create crons on Alice's app");
    rt.shutdown().await;
}

// ── Platform admin can still do everything ───────────────────────────

#[tokio::test]
async fn platform_admin_retains_full_access() {
    let rt = harness::TestRuntime::boot().await;
    ensure_admin(&rt).await;
    let (tok_emp, uid_emp) = employee(&rt, "emp@company.local").await;
    allow_install(&rt, uid_emp).await;
    rt.request_as(Method::POST, "/api/v1/apps", &tok_emp, Some(&manifest("managed"))).await;
    rt.request_as(
        Method::POST, "/api/v1/apps/managed/collections/tasks", &tok_emp,
        Some(&json!({ "title": "Task", "status": "open" })),
    ).await;

    // Platform admin (rt.token) can see everything regardless.
    let (s, body) = rt.request_as(Method::GET, "/api/v1/apps/managed/collections/tasks", &rt.token, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body.as_array().map(|a| a.len()), Some(1), "platform admin sees all data");
    rt.shutdown().await;
}

// ── Sad path: employee without platform:apps.create cannot install ────

#[tokio::test]
async fn employee_without_create_perm_cannot_install() {
    let rt = harness::TestRuntime::boot().await;
    ensure_admin(&rt).await;
    let (tok, _) = employee(&rt, "noperm@company.local").await;
    // No allow_install — this employee has zero platform permissions.
    let (s, _) = rt.request_as(Method::POST, "/api/v1/apps", &tok, Some(&manifest("denied"))).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "employee without platform:apps.create must be denied");
    rt.shutdown().await;
}

// ── Sad path: runAs on SA without act-as delegation ──────────────────

#[tokio::test]
async fn run_as_without_act_as_denied() {
    let rt = harness::TestRuntime::boot().await;
    ensure_admin(&rt).await;
    let (tok, uid) = employee(&rt, "dev7@company.local").await;
    allow_install(&rt, uid).await;
    rt.request_as(Method::POST, "/api/v1/apps", &tok, Some(&manifest("noact"))).await;

    // Create a SA but do NOT grant act-as to the employee.
    let (_, sa_body) = rt.post_json("/api/v1/service-accounts", &json!({ "slug": "noact_bot" })).await;
    let sa_id = sa_body["id"].as_str().unwrap();

    let (s, body) = rt.request_as(
        Method::POST, "/api/v1/apps/noact/crons", &tok,
        Some(&json!({ "name": "bad", "schedule": "0 * * * *", "runAs": sa_id })),
    ).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "runAs without act-as delegation must be denied: {body}");
    rt.shutdown().await;
}

// ── Sad path: revoked permission = lost access ───────────────────────

#[tokio::test]
async fn revoked_app_perm_loses_access() {
    let rt = harness::TestRuntime::boot().await;
    ensure_admin(&rt).await;
    let (tok, uid) = employee(&rt, "revoked@company.local").await;
    allow_install(&rt, uid).await;
    rt.request_as(Method::POST, "/api/v1/apps", &tok, Some(&manifest("revapp"))).await;

    // Write a record (should work).
    let (s, _) = rt.request_as(
        Method::POST, "/api/v1/apps/revapp/collections/tasks", &tok,
        Some(&json!({ "title": "before", "status": "open" })),
    ).await;
    assert_eq!(s, StatusCode::CREATED);

    // Admin revokes the employee's app-scoped role.
    sqlx::query("DELETE FROM rootcx_system.rbac_assignments WHERE user_id = $1")
        .bind(uid).execute(rt.pool()).await.unwrap();

    // Employee immediately loses data access (RLS).
    let (s, body) = rt.request_as(Method::GET, "/api/v1/apps/revapp/collections/tasks", &tok, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body.as_array().map(|a| a.len()), Some(0),
        "revoked permission must immediately cut data access");
    rt.shutdown().await;
}

// ── runAs with app-scoped SA (employee creates SA for their app) ─────

#[tokio::test]
async fn app_owner_uses_run_as_with_own_sa() {
    let rt = harness::TestRuntime::boot().await;
    ensure_admin(&rt).await;
    let (tok, uid) = employee(&rt, "dev6@company.local").await;
    allow_install(&rt, uid).await;
    rt.request_as(Method::POST, "/api/v1/apps", &tok, Some(&manifest("saapp"))).await;

    // Admin creates a SA (platform-level operation), grants it app:saapp:* perms,
    // and gives the employee act-as on it.
    let (_, sa_body) = rt.post_json("/api/v1/service-accounts", &json!({ "slug": "saapp_bot" })).await;
    let sa_id = sa_body["id"].as_str().unwrap();
    let sa_uid: Uuid = sa_id.parse().unwrap();

    // Give SA the same app-scoped perms (subset of the employee).
    sqlx::query("INSERT INTO rootcx_system.rbac_roles (name, permissions) VALUES ('sa_saapp', ARRAY['app:saapp:*']) ON CONFLICT (name) DO NOTHING")
        .execute(rt.pool()).await.unwrap();
    sqlx::query("INSERT INTO rootcx_system.rbac_assignments (user_id, role) VALUES ($1, 'sa_saapp') ON CONFLICT DO NOTHING")
        .bind(sa_uid).execute(rt.pool()).await.unwrap();

    // Grant act-as from employee to SA.
    rootcx_core::act_as::grant(rt.pool(), uid, sa_uid).await.unwrap();

    // Employee creates a cron owned by the SA.
    let (s, body) = rt.request_as(
        Method::POST, "/api/v1/apps/saapp/crons", &tok,
        Some(&json!({ "name": "sa_cron", "schedule": "0 * * * *", "runAs": sa_id })),
    ).await;
    assert_eq!(s, StatusCode::CREATED, "app owner with act-as must use runAs: {body}");

    // Verify the SA owns the cron.
    let owner: Uuid = sqlx::query_scalar(
        "SELECT created_by FROM rootcx_system.cron_schedules WHERE app_id = 'saapp' AND name = 'sa_cron'")
        .fetch_one(rt.pool()).await.unwrap();
    assert_eq!(owner, sa_uid, "cron must be owned by the SA");
    rt.shutdown().await;
}
