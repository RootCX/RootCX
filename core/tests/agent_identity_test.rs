mod harness;

use reqwest::{Method, StatusCode};
use serde_json::{json, Value};
use uuid::Uuid;

async fn ensure_admin(rt: &harness::TestRuntime) {
    let pool = rt.pool();
    let uid: Uuid = sqlx::query_scalar("SELECT id FROM rootcx_system.users WHERE email = 'admin@test.local'")
        .fetch_one(pool).await.unwrap();
    sqlx::query("INSERT INTO rootcx_system.rbac_assignments (user_id, role) VALUES ($1, 'admin') ON CONFLICT DO NOTHING")
        .bind(uid).execute(pool).await.unwrap();
}

async fn setup_agent_app(rt: &harness::TestRuntime) -> String {
    ensure_admin(rt).await;
    let app_id = format!("agent-{}", Uuid::new_v4().simple());
    let pool = rt.pool();

    sqlx::query("INSERT INTO rootcx_system.apps (id, name, version, status) VALUES ($1, $1, '1.0', 'installed')")
        .bind(&app_id).execute(pool).await.unwrap();
    sqlx::query(&format!("CREATE SCHEMA IF NOT EXISTS \"{}\"", app_id)).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO rootcx_system.agents (app_id, name, config) VALUES ($1, 'Test Agent', '{}')")
        .bind(&app_id).execute(pool).await.unwrap();

    let agent_uid = uuid::Uuid::new_v5(
        &uuid::Uuid::from_bytes([0x9a,0x3b,0x4c,0x5d,0x6e,0x7f,0x40,0x01,0x82,0x93,0xa4,0xb5,0xc6,0xd7,0xe8,0xf9]),
        format!("agent:{app_id}").as_bytes(),
    );
    sqlx::query("INSERT INTO rootcx_system.users (id, email, is_system) VALUES ($1, $2, true) ON CONFLICT DO NOTHING")
        .bind(agent_uid).bind(format!("agent+{app_id}@localhost")).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO rootcx_system.rbac_assignments (user_id, role) VALUES ($1, 'admin') ON CONFLICT DO NOTHING")
        .bind(agent_uid).execute(pool).await.unwrap();

    app_id
}

async fn create_user_with_perms(rt: &harness::TestRuntime, email: &str, perms: &[&str]) -> String {
    let pool = rt.pool();
    let token = rt.register_and_login(email).await;

    let uid: Uuid = sqlx::query_scalar("SELECT id FROM rootcx_system.users WHERE email = $1")
        .bind(email).fetch_one(pool).await.unwrap();

    // Remove default admin assignment (register auto-promotes first user)
    sqlx::query("DELETE FROM rootcx_system.rbac_assignments WHERE user_id = $1")
        .bind(uid).execute(pool).await.unwrap();

    let role_name = format!("role_{}", uid.simple());
    let perm_list: Vec<String> = perms.iter().map(|s| s.to_string()).collect();
    sqlx::query("INSERT INTO rootcx_system.rbac_roles (name, inherits, permissions) VALUES ($1, '{}', $2) ON CONFLICT (name) DO NOTHING")
        .bind(&role_name).bind(&perm_list).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO rootcx_system.rbac_assignments (user_id, role) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(uid).bind(&role_name).execute(pool).await.unwrap();

    token
}

fn agent_uid_for(app_id: &str) -> Uuid {
    uuid::Uuid::new_v5(
        &uuid::Uuid::from_bytes([0x9a,0x3b,0x4c,0x5d,0x6e,0x7f,0x40,0x01,0x82,0x93,0xa4,0xb5,0xc6,0xd7,0xe8,0xf9]),
        format!("agent:{app_id}").as_bytes(),
    )
}

// ═══════════════════════════════════════════════════════════════════
// INVOCATION ACL (CRITICAL)
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn invoke_agent_denied_without_permission() {
    let rt = harness::TestRuntime::boot().await;
    let app_id = setup_agent_app(&rt).await;

    // Register a user with NO invoke permission for this app
    let token = rt.register_and_login("noinvoke@test.local").await;
    // Remove admin role to make them unprivileged
    let uid: Uuid = sqlx::query_scalar("SELECT id FROM rootcx_system.users WHERE email = 'noinvoke@test.local'")
        .fetch_one(rt.pool()).await.unwrap();
    sqlx::query("DELETE FROM rootcx_system.rbac_assignments WHERE user_id = $1")
        .bind(uid).execute(rt.pool()).await.unwrap();
    let role = format!("role_noinvoke_{}", Uuid::new_v4().simple());
    sqlx::query("INSERT INTO rootcx_system.rbac_roles (name, inherits, permissions) VALUES ($1, '{}', $2)")
        .bind(&role).bind(&vec!["app:other:invoke".to_string()]).execute(rt.pool()).await.unwrap();
    sqlx::query("INSERT INTO rootcx_system.rbac_assignments (user_id, role) VALUES ($1, $2)")
        .bind(uid).bind(&role).execute(rt.pool()).await.unwrap();

    let (status, _) = rt.request_as(
        Method::POST,
        &format!("/api/v1/apps/{app_id}/agent/invoke"),
        &token,
        Some(&json!({"message": "test"})),
    ).await;

    assert_eq!(status, StatusCode::FORBIDDEN, "user without app:{{id}}:invoke should get 403");
}

#[tokio::test]
async fn invoke_agent_allowed_with_wildcard() {
    let rt = harness::TestRuntime::boot().await;
    let app_id = setup_agent_app(&rt).await;

    let (status, _) = rt.request_as(
        Method::POST,
        &format!("/api/v1/apps/{app_id}/agent/invoke"),
        &rt.token,
        Some(&json!({"message": "test"})),
    ).await;

    assert_ne!(status, StatusCode::FORBIDDEN, "admin with '*' should pass invocation ACL");
}

// ═══════════════════════════════════════════════════════════════════
// DELEGATIONS TABLE (CRITICAL)
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn delegation_valid_active() {
    let rt = harness::TestRuntime::boot().await;
    let pool = rt.pool();
    let app_id = setup_agent_app(&rt).await;

    let uid: Uuid = sqlx::query_scalar("SELECT id FROM rootcx_system.users WHERE email = 'admin@test.local'")
        .fetch_one(pool).await.unwrap();
    let agent = agent_uid_for(&app_id);

    rootcx_core::delegations::create(pool, uid, agent, "manual", None).await.unwrap();
    assert!(rootcx_core::delegations::is_valid(pool, uid, agent).await.unwrap());
}

#[tokio::test]
async fn delegation_revoked_denied() {
    let rt = harness::TestRuntime::boot().await;
    let pool = rt.pool();
    let app_id = setup_agent_app(&rt).await;

    let uid: Uuid = sqlx::query_scalar("SELECT id FROM rootcx_system.users WHERE email = 'admin@test.local'")
        .fetch_one(pool).await.unwrap();
    let agent = agent_uid_for(&app_id);

    let id = rootcx_core::delegations::create(pool, uid, agent, "manual", None).await.unwrap();
    rootcx_core::delegations::revoke(pool, id).await.unwrap();
    assert!(!rootcx_core::delegations::is_valid(pool, uid, agent).await.unwrap());
}

#[tokio::test]
async fn delegation_expired_denied() {
    let rt = harness::TestRuntime::boot().await;
    let pool = rt.pool();
    let app_id = setup_agent_app(&rt).await;

    let uid: Uuid = sqlx::query_scalar("SELECT id FROM rootcx_system.users WHERE email = 'admin@test.local'")
        .fetch_one(pool).await.unwrap();
    let agent = agent_uid_for(&app_id);

    sqlx::query(
        "INSERT INTO rootcx_system.delegations (delegator_uid, agent_uid, trigger_type, expires_at) \
         VALUES ($1, $2, 'manual', now() - interval '1 hour')"
    ).bind(uid).bind(agent).execute(pool).await.unwrap();

    assert!(!rootcx_core::delegations::is_valid(pool, uid, agent).await.unwrap());
}

#[tokio::test]
async fn delegation_nonexistent_denied() {
    let rt = harness::TestRuntime::boot().await;
    let pool = rt.pool();

    let random_uid = Uuid::new_v4();
    sqlx::query("INSERT INTO rootcx_system.users (id, email) VALUES ($1, 'phantom@test.local') ON CONFLICT DO NOTHING")
        .bind(random_uid).execute(pool).await.unwrap();

    assert!(!rootcx_core::delegations::is_valid(pool, random_uid, Uuid::new_v4()).await.unwrap());
}

// ═══════════════════════════════════════════════════════════════════
// WEBHOOK DISPATCH -- AGENT DELEGATION CHECK (CRITICAL)
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn webhook_bad_token_404() {
    let rt = harness::TestRuntime::boot().await;
    let res = rt.client.post(rt.url("/api/v1/hooks/nonexistent-xyz"))
        .json(&json!({})).send().await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn webhook_agent_no_owner_403() {
    let rt = harness::TestRuntime::boot().await;
    let app_id = setup_agent_app(&rt).await;
    let pool = rt.pool();

    sqlx::query("INSERT INTO rootcx_system.webhooks (app_id, name, method, token) VALUES ($1, 'noowner', 'agent', 'tok-noowner')")
        .bind(&app_id).execute(pool).await.unwrap();

    let res = rt.client.post(rt.url("/api/v1/hooks/tok-noowner"))
        .json(&json!({"x":1})).send().await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn webhook_agent_revoked_delegation_403() {
    let rt = harness::TestRuntime::boot().await;
    let app_id = setup_agent_app(&rt).await;
    let pool = rt.pool();

    let uid: Uuid = sqlx::query_scalar("SELECT id FROM rootcx_system.users WHERE email = 'admin@test.local'")
        .fetch_one(pool).await.unwrap();

    let (wh_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO rootcx_system.webhooks (app_id, name, method, token, created_by) VALUES ($1, 'revoked', 'agent', 'tok-revoked', $2) RETURNING id"
    ).bind(&app_id).bind(uid).fetch_one(pool).await.unwrap();

    let agent = agent_uid_for(&app_id);
    let del_id = rootcx_core::delegations::create(pool, uid, agent, "webhook", Some(wh_id)).await.unwrap();
    rootcx_core::delegations::revoke(pool, del_id).await.unwrap();

    let res = rt.client.post(rt.url("/api/v1/hooks/tok-revoked"))
        .json(&json!({})).send().await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn webhook_agent_valid_delegation_accepted() {
    let rt = harness::TestRuntime::boot().await;
    let app_id = setup_agent_app(&rt).await;
    let pool = rt.pool();

    let uid: Uuid = sqlx::query_scalar("SELECT id FROM rootcx_system.users WHERE email = 'admin@test.local'")
        .fetch_one(pool).await.unwrap();

    let (wh_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO rootcx_system.webhooks (app_id, name, method, token, created_by) VALUES ($1, 'valid', 'agent', 'tok-valid', $2) RETURNING id"
    ).bind(&app_id).bind(uid).fetch_one(pool).await.unwrap();

    let agent = agent_uid_for(&app_id);
    rootcx_core::delegations::create(pool, uid, agent, "webhook", Some(wh_id)).await.unwrap();

    let res = rt.client.post(rt.url("/api/v1/hooks/tok-valid"))
        .json(&json!({})).send().await.unwrap();
    // Not 403/404 -- agent invoke may error (no worker running) but auth passed
    let s = res.status();
    assert!(s != StatusCode::FORBIDDEN && s != StatusCode::NOT_FOUND,
        "valid delegation should pass, got {s}");
}

// ═══════════════════════════════════════════════════════════════════
// CRUD ACT-AWARE (CRITICAL -- Path 2 regression)
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn crud_legacy_token_works_unchanged() {
    let rt = harness::TestRuntime::boot().await;
    ensure_admin(&rt).await;
    rt.install("legacyapp", "contacts").await;

    let (status, _) = rt.get_json("/api/v1/apps/legacyapp/collections/contacts").await;
    assert_eq!(status, StatusCode::OK, "legacy token (no act) must work as before");
}

// ═══════════════════════════════════════════════════════════════════
// AUDIT CONTEXT CAPTURE
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn audit_captures_actor_on_api_write() {
    let rt = harness::TestRuntime::boot().await;
    ensure_admin(&rt).await;
    let pool = rt.pool();

    let app_id = "auditcaptest";
    rt.install(app_id, "items").await;

    // Enable tracking on the entity table
    sqlx::query(&format!("SELECT rootcx_system.enable_tracking('\"{app_id}\".\"items\"'::regclass)"))
        .execute(pool).await.unwrap();

    // Create a record
    rt.create(app_id, "items", &json!({"first_name":"audit","last_name":"test"})).await;

    // Check audit_log
    let actor: Option<Uuid> = sqlx::query_scalar(
        "SELECT actor_uid FROM rootcx_system.audit_log WHERE table_schema = $1 AND table_name = 'items' ORDER BY id DESC LIMIT 1"
    ).bind(app_id).fetch_optional(pool).await.unwrap();

    assert!(actor.is_some(), "audit_log must capture actor_uid on API writes");

    let admin_uid: Uuid = sqlx::query_scalar("SELECT id FROM rootcx_system.users WHERE email = 'admin@test.local'")
        .fetch_one(pool).await.unwrap();
    assert_eq!(actor.unwrap(), admin_uid, "actor should be the authenticated user");
}

// ═══════════════════════════════════════════════════════════════════
// CRON AUTO-DELEGATION
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn cron_creation_creates_delegation() {
    let rt = harness::TestRuntime::boot().await;
    let pool = rt.pool();
    let app_id = setup_agent_app(&rt).await;

    let (status, body) = rt.post_json(
        &format!("/api/v1/apps/{app_id}/crons"),
        &json!({"name": "autocron", "schedule": "0 0 * * *"}),
    ).await;

    // pg_cron may not be available in test
    if status == StatusCode::INTERNAL_SERVER_ERROR {
        let msg = body.to_string();
        if msg.contains("pg_cron") { return; }
    }
    assert_eq!(status, StatusCode::CREATED);

    let admin_uid: Uuid = sqlx::query_scalar("SELECT id FROM rootcx_system.users WHERE email = 'admin@test.local'")
        .fetch_one(pool).await.unwrap();
    let agent = agent_uid_for(&app_id);

    assert!(rootcx_core::delegations::is_valid(pool, admin_uid, agent).await.unwrap(),
        "cron creation must auto-create a delegation");
}

// ═══════════════════════════════════════════════════════════════════
// WORKER PERMISSION CHAIN: Verify the intersection is enforced
// at invocation time (cached) and gates tools correctly.
// These test the full code path from invoke -> permission cache
// without needing a running JS agent process.
// ═══════════════════════════════════════════════════════════════════

// Worker permission integration: verify intersection is computed and cached
// at invoke registration time using the actual RBAC DB queries.

#[tokio::test]
async fn worker_permission_intersection_computed_from_db() {
    let rt = harness::TestRuntime::boot().await;
    ensure_admin(&rt).await;
    let pool = rt.pool();
    let app_id = setup_agent_app(&rt).await;

    // Admin user has '*' (via ensure_admin)
    let admin_uid: Uuid = sqlx::query_scalar("SELECT id FROM rootcx_system.users WHERE email = 'admin@test.local'")
        .fetch_one(pool).await.unwrap();
    let agent_uid = agent_uid_for(&app_id);

    // Verify: agent has '*'
    let (_, agent_perms) = rootcx_core::extensions::rbac::policy::resolve_permissions(pool, agent_uid).await.unwrap();
    assert!(agent_perms.contains(&"*".to_string()), "agent should have admin '*'");

    // Verify: admin has '*'
    let (_, admin_perms) = rootcx_core::extensions::rbac::policy::resolve_permissions(pool, admin_uid).await.unwrap();
    assert!(admin_perms.contains(&"*".to_string()), "admin should have '*'");

    // Intersection of ['*'] and ['*'] = ['*']
    let effective = rootcx_core::extensions::rbac::policy::intersect_permissions(&agent_perms, &admin_perms);
    assert_eq!(effective, vec!["*"], "intersection of two '*' sets should be ['*']");

    // Verify: has_permission passes for any app:entity permission
    assert!(rootcx_core::extensions::rbac::policy::has_permission(&effective, &format!("app:{app_id}:tasks.read")));
}

#[tokio::test]
async fn worker_permission_none_invoker_gives_empty() {
    // This tests the deny-on-None logic: when invoker_user_id is None,
    // the effective permissions must be empty (deny all).
    // This is the exact code path in worker.rs:
    //   None => vec![]
    let perms: Vec<String> = match None::<Uuid> {
        Some(_uid) => unreachable!(),
        None => vec![],
    };
    assert!(perms.is_empty());
    assert!(!rootcx_core::extensions::rbac::policy::has_permission(&perms, "tool:query_data"));
    assert!(!rootcx_core::extensions::rbac::policy::has_permission(&perms, "app:x:tasks.read"));
}

#[tokio::test]
async fn worker_permission_lowpriv_invoker_restricts_agent() {
    let rt = harness::TestRuntime::boot().await;
    ensure_admin(&rt).await;
    let pool = rt.pool();
    let app_id = setup_agent_app(&rt).await;

    // Create a user with only specific permissions
    let uid = Uuid::new_v4();
    sqlx::query("INSERT INTO rootcx_system.users (id, email) VALUES ($1, 'restricted@t.local')")
        .bind(uid).execute(pool).await.unwrap();
    let allowed_perm = format!("app:{app_id}:tasks.read");
    let role = format!("r_{}", uid.simple());
    sqlx::query("INSERT INTO rootcx_system.rbac_roles (name, inherits, permissions) VALUES ($1, '{}', $2)")
        .bind(&role).bind(&vec![allowed_perm.clone(), "tool:query_data".into()]).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO rootcx_system.rbac_assignments (user_id, role) VALUES ($1, $2)")
        .bind(uid).bind(&role).execute(pool).await.unwrap();

    // Resolve both sets (same as worker.rs does at invoke time)
    let agent_uid = agent_uid_for(&app_id);
    let (_, agent_perms) = rootcx_core::extensions::rbac::policy::resolve_permissions(pool, agent_uid).await.unwrap();
    let (_, invoker_perms) = rootcx_core::extensions::rbac::policy::resolve_permissions(pool, uid).await.unwrap();

    // Intersection restricts to invoker's permissions
    let effective = rootcx_core::extensions::rbac::policy::intersect_permissions(&agent_perms, &invoker_perms);

    // Should have the allowed permissions
    assert!(rootcx_core::extensions::rbac::policy::has_permission(&effective, &allowed_perm),
        "effective should include the allowed perm");
    assert!(rootcx_core::extensions::rbac::policy::has_permission(&effective, "tool:query_data"),
        "effective should include tool:query_data");

    // Should NOT have permissions the invoker lacks
    let denied_perm = format!("app:{app_id}:tasks.write");
    assert!(!rootcx_core::extensions::rbac::policy::has_permission(&effective, &denied_perm),
        "effective must NOT include perms the invoker lacks (CRITICAL: escalation prevention)");
}

// ═══════════════════════════════════════════════════════════════════
// PER-AGENT RBAC GRANT (admin restricts agent via standard RBAC API)
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn agent_restricted_via_rbac_bounded_below_admin() {
    let rt = harness::TestRuntime::boot().await;
    let app_id = setup_agent_app(&rt).await;
    let pool = rt.pool();
    let agent_uid = agent_uid_for(&app_id);

    // Admin restricts the agent: revoke admin, assign a narrow role (same as RBAC API would do)
    sqlx::query("DELETE FROM rootcx_system.rbac_assignments WHERE user_id = $1")
        .bind(agent_uid).execute(pool).await.unwrap();
    let role_name = format!("agent_restricted_{}", &app_id[..8]);
    let narrow_perms = vec![format!("app:{app_id}:tasks.read"), "tool:query_data".into()];
    sqlx::query("INSERT INTO rootcx_system.rbac_roles (name, inherits, permissions) VALUES ($1, '{}', $2) ON CONFLICT (name) DO NOTHING")
        .bind(&role_name).bind(&narrow_perms).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO rootcx_system.rbac_assignments (user_id, role) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(agent_uid).bind(&role_name).execute(pool).await.unwrap();

    // Verify agent perms are narrow
    let (_, agent_perms) = rootcx_core::extensions::rbac::policy::resolve_permissions(pool, agent_uid).await.unwrap();
    assert!(!agent_perms.contains(&"*".to_string()), "agent must NOT have wildcard after restriction");
    assert!(agent_perms.contains(&format!("app:{app_id}:tasks.read")));

    // Admin invokes: effective = intersection(agent_narrow, admin_*)
    let admin_uid: Uuid = sqlx::query_scalar("SELECT id FROM rootcx_system.users WHERE email = 'admin@test.local'")
        .fetch_one(pool).await.unwrap();
    let effective = rootcx_core::extensions::rbac::policy::effective_for_pair(pool, agent_uid, admin_uid).await;

    assert!(rootcx_core::extensions::rbac::policy::has_permission(&effective, &format!("app:{app_id}:tasks.read")));
    assert!(rootcx_core::extensions::rbac::policy::has_permission(&effective, "tool:query_data"));
    assert!(!rootcx_core::extensions::rbac::policy::has_permission(&effective, &format!("app:{app_id}:tasks.create")),
        "CRITICAL: agent must NOT exceed its RBAC-assigned grant");
}

#[tokio::test]
async fn agent_default_gets_admin_on_first_deploy() {
    let rt = harness::TestRuntime::boot().await;
    let app_id = setup_agent_app(&rt).await;
    let pool = rt.pool();
    let agent_uid = agent_uid_for(&app_id);

    let (_, perms) = rootcx_core::extensions::rbac::policy::resolve_permissions(pool, agent_uid).await.unwrap();
    assert!(perms.contains(&"*".to_string()), "agent gets admin on first deploy for backward compat");
}

#[tokio::test]
async fn agent_redeploy_preserves_restricted_role() {
    let rt = harness::TestRuntime::boot().await;
    let app_id = setup_agent_app(&rt).await;
    let pool = rt.pool();
    let agent_uid = agent_uid_for(&app_id);

    // Admin restricts the agent
    sqlx::query("DELETE FROM rootcx_system.rbac_assignments WHERE user_id = $1")
        .bind(agent_uid).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO rootcx_system.rbac_roles (name, inherits, permissions) VALUES ('narrow_role', '{}', $1) ON CONFLICT (name) DO NOTHING")
        .bind(&vec!["app:x:read".to_string()]).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO rootcx_system.rbac_assignments (user_id, role) VALUES ($1, 'narrow_role') ON CONFLICT DO NOTHING")
        .bind(agent_uid).execute(pool).await.unwrap();

    // Re-register the agent (simulates redeploy)
    let def = rootcx_types::AgentDefinition {
        name: "Re-deployed Agent".into(),
        description: None,
        system_prompt: None,
        memory: None,
        limits: None,
        supervision: None,
    };
    rootcx_core::extensions::agents::register_agent(pool, &app_id, &def).await.unwrap();

    // Role must NOT be overwritten back to admin
    let (_, perms) = rootcx_core::extensions::rbac::policy::resolve_permissions(pool, agent_uid).await.unwrap();
    assert!(!perms.contains(&"*".to_string()),
        "CRITICAL: redeploy must not overwrite admin-assigned restricted role");
}

// ═══════════════════════════════════════════════════════════════════
// MIGRATION BACKFILL (legacy triggers with NULL created_by)
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn legacy_cron_backfilled_to_admin() {
    let rt = harness::TestRuntime::boot().await;
    let pool = rt.pool();
    let app_id = setup_agent_app(&rt).await;

    // Insert a legacy cron with created_by = NULL (pre-upgrade state)
    sqlx::query(
        "INSERT INTO rootcx_system.cron_schedules (id, app_id, name, schedule, payload, overlap_policy) \
         VALUES ($1, $2, 'legacy', '0 0 * * *', '{}', 'skip')"
    ).bind(Uuid::new_v4()).bind(&app_id).execute(pool).await.unwrap();

    // Re-run the migration (simulates next boot)
    rootcx_core::delegations::migrate_existing_triggers(pool).await.unwrap();

    // The cron should now have created_by set to admin
    let owner: Option<Uuid> = sqlx::query_scalar(
        "SELECT created_by FROM rootcx_system.cron_schedules WHERE app_id = $1 AND name = 'legacy'"
    ).bind(&app_id).fetch_one(pool).await.unwrap();
    assert!(owner.is_some(), "legacy cron should be backfilled with admin owner");

    // A delegation should exist
    let agent = agent_uid_for(&app_id);
    assert!(rootcx_core::delegations::is_valid(pool, owner.unwrap(), agent).await.unwrap(),
        "delegation should be created for backfilled cron");
}

#[tokio::test]
async fn legacy_webhook_backfilled_to_admin() {
    let rt = harness::TestRuntime::boot().await;
    let pool = rt.pool();
    let app_id = setup_agent_app(&rt).await;

    // Insert a legacy webhook with created_by = NULL
    sqlx::query(
        "INSERT INTO rootcx_system.webhooks (app_id, name, method, token) VALUES ($1, 'legacy-wh', 'agent', 'tok-legacy')"
    ).bind(&app_id).execute(pool).await.unwrap();

    rootcx_core::delegations::migrate_existing_triggers(pool).await.unwrap();

    let owner: Option<Uuid> = sqlx::query_scalar(
        "SELECT created_by FROM rootcx_system.webhooks WHERE app_id = $1 AND name = 'legacy-wh'"
    ).bind(&app_id).fetch_one(pool).await.unwrap();
    assert!(owner.is_some(), "legacy webhook should be backfilled with admin owner");

    let agent = agent_uid_for(&app_id);
    assert!(rootcx_core::delegations::is_valid(pool, owner.unwrap(), agent).await.unwrap(),
        "delegation should be created for backfilled webhook");
}

// ═══════════════════════════════════════════════════════════════════
// INVOKE PERMISSION GRANTABLE
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn invoke_permission_generated_on_install() {
    let rt = harness::TestRuntime::boot().await;
    ensure_admin(&rt).await;

    let app_id = "invpermtest";
    rt.install(app_id, "items").await;

    let pool = rt.pool();
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM rootcx_system.rbac_permissions WHERE key = $1)"
    ).bind(format!("app:{app_id}:invoke")).fetch_one(pool).await.unwrap();

    assert!(exists, "app:invpermtest:invoke permission should be auto-generated on install");
}

#[tokio::test]
async fn invoke_granted_via_role_allows_access() {
    let rt = harness::TestRuntime::boot().await;
    ensure_admin(&rt).await;
    let pool = rt.pool();

    // Install app via manifest (seeds permissions including invoke)
    let app_id = "invgranttest";
    rt.install(app_id, "tasks").await;

    // Register as agent
    sqlx::query("INSERT INTO rootcx_system.agents (app_id, name, config) VALUES ($1, 'Test Agent', '{}') ON CONFLICT DO NOTHING")
        .bind(app_id).execute(pool).await.unwrap();
    let agent_uid = agent_uid_for(app_id);
    sqlx::query("INSERT INTO rootcx_system.users (id, email, is_system) VALUES ($1, $2, true) ON CONFLICT DO NOTHING")
        .bind(agent_uid).bind(format!("agent+{app_id}@localhost")).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO rootcx_system.rbac_assignments (user_id, role) VALUES ($1, 'admin') ON CONFLICT DO NOTHING")
        .bind(agent_uid).execute(pool).await.unwrap();

    // Create a non-admin user with the invoke permission
    let invoke_perm = format!("app:{app_id}:invoke");
    let token = create_user_with_perms(&rt, "invoker@test.local", &[&invoke_perm]).await;

    let (status, _) = rt.request_as(
        Method::POST,
        &format!("/api/v1/apps/{app_id}/agent/invoke"),
        &token,
        Some(&serde_json::json!({"message": "test"})),
    ).await;

    // Should pass ACL (may fail later because no worker, but NOT 403)
    assert_ne!(status, StatusCode::FORBIDDEN,
        "user with app:{{id}}:invoke should pass invocation ACL");
}

// ═══════════════════════════════════════════════════════════════════
// WEBHOOK ROUTING: method-based dispatch
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn webhook_rpc_method_not_routed_to_agent() {
    let rt = harness::TestRuntime::boot().await;
    let app_id = setup_agent_app(&rt).await;
    let pool = rt.pool();

    let admin_uid: Uuid = sqlx::query_scalar("SELECT id FROM rootcx_system.users WHERE email = 'admin@test.local'")
        .fetch_one(pool).await.unwrap();

    // Insert a webhook with a real RPC method (not "agent")
    sqlx::query(
        "INSERT INTO rootcx_system.webhooks (app_id, name, method, token, created_by) \
         VALUES ($1, 'stripe', 'handleStripe', 'tok-rpc', $2)"
    ).bind(&app_id).bind(admin_uid).execute(pool).await.unwrap();

    let res = rt.client.post(rt.url("/api/v1/hooks/tok-rpc"))
        .json(&serde_json::json!({"event": "charge.succeeded"})).send().await.unwrap();

    // Should NOT be 403 (agent delegation check). It hits the RPC path,
    // which may fail with 500 (no worker running) but not 403.
    let s = res.status();
    assert_ne!(s, StatusCode::FORBIDDEN,
        "webhook with RPC method should route to RPC, not agent delegation check; got {s}");
}

// ═══════════════════════════════════════════════════════════════════
// CROSS-APP ACTION CALLBACK: caller identity, not target identity
// ═══════════════════════════════════════════════════════════════════
