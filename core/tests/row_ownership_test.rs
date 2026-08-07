//! Row ownership — the `.own` scope, end to end.
//!
//! A field marked `owner: true` makes the Core mint `{entity}.{action}.own`
//! permission keys and generate RLS policies confining their holder to rows whose
//! owner column equals the caller. These tests assert the observable result
//! through the HTTP API, because the guarantee is a property of the generated SQL
//! running as the restricted executor role — something no unit test can reach.

mod harness;

use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use uuid::Uuid;

/// One entity owned through a `uuid` column. Not an `entity_link`: that shape
/// gets a foreign-key index for free, so the bare `uuid` is the case where the
/// ownership index and the GUC cast have to carry themselves.
async fn install_owned(rt: &harness::TestRuntime) {
    rt.install_manifest(&json!({
        "appId": "hr", "name": "hr", "version": "1.0.0",
        "dataContract": [{ "entityName": "profile", "fields": [
            { "name": "user_id", "type": "uuid", "owner": true },
            { "name": "nickname", "type": "text" },
        ]}]
    })).await;
}

async fn user_with(rt: &harness::TestRuntime, email: &str, perms: &[&str]) -> (String, Uuid) {
    let pool = rt.pool();
    let token = rt.register_and_login(email).await;
    let uid: Uuid = sqlx::query_scalar("SELECT id FROM rootcx_system.users WHERE email = $1")
        .bind(email).fetch_one(pool).await.unwrap();
    sqlx::query("DELETE FROM rootcx_system.rbac_assignments WHERE user_id = $1")
        .bind(uid).execute(pool).await.unwrap();
    let role = format!("role_{}", uid.simple());
    let perms: Vec<String> = perms.iter().map(ToString::to_string).collect();
    sqlx::query(
        "INSERT INTO rootcx_system.rbac_roles (name, inherits, permissions) VALUES ($1, '{}', $2) \
         ON CONFLICT (name) DO UPDATE SET permissions = EXCLUDED.permissions",
    ).bind(&role).bind(&perms).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO rootcx_system.rbac_assignments (user_id, role) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(uid).bind(&role).execute(pool).await.unwrap();
    (token, uid)
}

/// A row belonging to `owner`, created by the admin so the fixture never depends
/// on the very policy under test.
async fn row_of(rt: &harness::TestRuntime, owner: Uuid, nickname: &str) -> String {
    let body = rt.create("hr", "profile", &json!({"user_id": owner, "nickname": nickname})).await;
    body["id"].as_str().expect("created record returns its id").to_string()
}

/// The whole point of the feature: `.own` sees exactly one row out of several,
/// and the rows it cannot see are invisible rather than forbidden.
#[tokio::test]
async fn own_read_confines_to_the_caller_s_rows() {
    let rt = harness::TestRuntime::boot().await;
    install_owned(&rt).await;

    let (tok, mine) = user_with(&rt, "jean@t.local", &["app:hr:profile.read.own"]).await;
    let (_, theirs) = user_with(&rt, "marie@t.local", &[]).await;
    row_of(&rt, mine, "jean").await;
    let other = row_of(&rt, theirs, "marie").await;
    row_of(&rt, Uuid::nil(), "nobody").await;

    let (s, body) = rt.request_as(Method::GET, "/api/v1/apps/hr/collections/profile", &tok, None).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let rows = body.as_array().expect("list returns an array");
    assert_eq!(rows.len(), 1, "'.own' must see only the caller's row: {body}");
    assert_eq!(rows[0]["nickname"], json!("jean"));

    let (s, body) = rt.request_as(
        Method::GET, &format!("/api/v1/apps/hr/collections/profile/{other}"), &tok, None,
    ).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "another owner's row is invisible, not forbidden: {body}");

    rt.shutdown().await;
}

/// `.own` is a lesser key, never a second door to the whole table: holding it
/// must not let a caller reach, reassign, or delete anything but its own row.
#[tokio::test]
async fn own_write_cannot_escape_the_caller_s_rows() {
    let rt = harness::TestRuntime::boot().await;
    install_owned(&rt).await;

    let perms = ["create", "read", "update", "delete"]
        .map(|action| format!("app:hr:profile.{action}.own"));
    let perms: Vec<&str> = perms.iter().map(String::as_str).collect();
    let (tok, mine) = user_with(&rt, "jean@t.local", &perms).await;
    let (_, theirs) = user_with(&rt, "marie@t.local", &[]).await;
    let own = row_of(&rt, mine, "jean").await;
    let other = row_of(&rt, theirs, "marie").await;

    for (label, method, path, body, expected) in [
        // Own row: full CRUD, so the confinement below is a scope and not a lockout.
        ("update own row", Method::PATCH, own.clone(), Some(json!({"nickname": "jeannot"})), StatusCode::OK),
        // Someone else's row: invisible, so USING filters it before any check.
        ("update another's row", Method::PATCH, other.clone(), Some(json!({"nickname": "stolen"})), StatusCode::NOT_FOUND),
        ("delete another's row", Method::DELETE, other.clone(), None, StatusCode::NOT_FOUND),
        // Own row, reassigned away: visible, so WITH CHECK is what must refuse it.
        ("give own row away", Method::PATCH, own.clone(), Some(json!({"user_id": theirs})), StatusCode::FORBIDDEN),
        ("delete own row", Method::DELETE, own.clone(), None, StatusCode::OK),
    ] {
        let path = format!("/api/v1/apps/hr/collections/profile/{path}");
        let (s, response) = rt.request_as(method, &path, &tok, body.as_ref()).await;
        assert_eq!(s, expected, "{label}: {response}");
    }

    // Creating for someone else is the same escape through INSERT.
    let (s, body) = rt.request_as(
        Method::POST, "/api/v1/apps/hr/collections/profile", &tok,
        Some(&json!({"user_id": theirs, "nickname": "planted"})),
    ).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "create must not plant a row on another owner: {body}");

    rt.shutdown().await;
}

/// "May see everyone, may only edit their own" — a directory, the most ordinary
/// reason to reach for `.own` at all.
///
/// This mix is what makes the row-scoped UPDATE need its own `WITH CHECK`. When
/// reads are also confined, an attempted transfer is already stopped upstream, so
/// dropping that clause changes nothing and looks safe. Here the caller can see
/// every row, and the clause is the only thing left refusing to hand a row over.
#[tokio::test]
async fn reading_every_row_does_not_widen_what_own_may_write() {
    let rt = harness::TestRuntime::boot().await;
    install_owned(&rt).await;

    let (tok, mine) = user_with(
        &rt, "jean@t.local", &["app:hr:profile.read", "app:hr:profile.update.own"],
    ).await;
    let (_, theirs) = user_with(&rt, "marie@t.local", &[]).await;
    let own = row_of(&rt, mine, "jean").await;
    let other = row_of(&rt, theirs, "marie").await;

    let (_, body) = rt.request_as(Method::GET, "/api/v1/apps/hr/collections/profile", &tok, None).await;
    assert_eq!(body.as_array().map(Vec::len), Some(2), "the unscoped read key still sees both: {body}");

    for (label, id, patch, expected) in [
        ("edit own row", &own, json!({"nickname": "jeannot"}), StatusCode::OK),
        // Visible through the unscoped read key, but not writable.
        ("edit a visible row it does not own", &other, json!({"nickname": "stolen"}), StatusCode::NOT_FOUND),
        // Visible and writable, but the new row would belong to someone else.
        ("hand its own row over", &own, json!({"user_id": theirs}), StatusCode::FORBIDDEN),
    ] {
        let path = format!("/api/v1/apps/hr/collections/profile/{id}");
        let (s, response) = rt.request_as(Method::PATCH, &path, &tok, Some(&patch)).await;
        assert_eq!(s, expected, "{label}: {response}");
    }

    let still_theirs: Uuid = sqlx::query_scalar("SELECT user_id FROM hr.profile WHERE id = $1")
        .bind(Uuid::parse_str(&other).unwrap()).fetch_one(rt.pool()).await.unwrap();
    assert_eq!(still_theirs, theirs, "no write reached another owner's row");

    rt.shutdown().await;
}

/// The row-scoped policies are PERMISSIVE, so Postgres ORs them with the
/// unscoped four. An existing grant must therefore be untouched by the feature —
/// this is the regression guard for every app already in production.
#[tokio::test]
async fn unscoped_permission_still_sees_every_row() {
    let rt = harness::TestRuntime::boot().await;
    install_owned(&rt).await;

    let (tok, mine) = user_with(&rt, "boss@t.local", &["app:hr:profile.read"]).await;
    row_of(&rt, mine, "boss").await;
    row_of(&rt, Uuid::new_v4(), "someone").await;

    let (s, body) = rt.request_as(Method::GET, "/api/v1/apps/hr/collections/profile", &tok, None).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body.as_array().map(Vec::len), Some(2), "unscoped read is unaffected by ownership: {body}");

    rt.shutdown().await;
}

/// `.own` keys exist only where a row can be owned, and the row-scoped policies
/// with them. An entity declaring no owner keeps exactly its pre-feature SQL,
/// which is what makes upgrading an existing tenant a no-op.
#[tokio::test]
async fn ownership_artifacts_track_the_declaration() {
    let rt = harness::TestRuntime::boot().await;
    install_owned(&rt).await;
    rt.install("crm", "contacts").await;

    let own_keys: Vec<String> = sqlx::query_scalar(
        "SELECT key FROM rootcx_system.rbac_permissions WHERE key LIKE '%.own' ORDER BY key",
    ).fetch_all(rt.pool()).await.unwrap();
    assert_eq!(
        own_keys,
        ["create", "delete", "read", "update"].map(|a| format!("app:hr:profile.{a}.own")),
        "'.own' keys are minted for the owned entity and for nothing else",
    );

    let policies: Vec<(String, i64)> = sqlx::query_as(
        "SELECT tablename, count(*) FROM pg_policies \
          WHERE policyname LIKE 'rootcx_rls_%_own' AND schemaname IN ('hr', 'crm') \
          GROUP BY tablename",
    ).fetch_all(rt.pool()).await.unwrap();
    assert_eq!(policies, vec![("profile".to_string(), 4)], "only the owned table gains policies");

    // A confined caller filters every read by owner, so the planner must be able
    // to use an index for it. Two ways to lose that, both invisible in behaviour:
    // no index on the column, or a predicate written `user_id::text = $guc`, which
    // is not indexable. Asserted on the plan rather than on either cause.
    for _ in 0..2_000 {
        sqlx::query("INSERT INTO hr.profile (user_id) VALUES (gen_random_uuid())")
            .execute(rt.pool()).await.unwrap();
    }
    sqlx::query("ANALYZE hr.profile").execute(rt.pool()).await.unwrap();
    let plan: Vec<String> = sqlx::query_scalar(
        "EXPLAIN SELECT * FROM hr.profile \
          WHERE user_id = (SELECT nullif(current_setting('rootcx.user_id', true), ''))::uuid",
    ).fetch_all(rt.pool()).await.unwrap();
    let plan = plan.join("\n");
    assert!(
        plan.contains("Index Scan") || plan.contains("Bitmap Index Scan"),
        "a confined read must not scan the whole table; plan was:\n{plan}",
    );

    rt.shutdown().await;
}

/// Ownership is a manifest declaration, so a redeploy can add or remove it. Both
/// directions must reconcile: a stranded policy would keep confining a table that
/// no longer claims to be owned, and a stranded key would grant a scope that has
/// no policy to honour it.
#[tokio::test]
async fn dropping_the_declaration_removes_the_confinement() {
    let rt = harness::TestRuntime::boot().await;
    install_owned(&rt).await;

    let (tok, mine) = user_with(&rt, "jean@t.local", &["app:hr:profile.read.own"]).await;
    row_of(&rt, mine, "jean").await;
    row_of(&rt, Uuid::new_v4(), "someone").await;

    rt.install_manifest(&json!({
        "appId": "hr", "name": "hr", "version": "1.0.1",
        "dataContract": [{ "entityName": "profile", "fields": [
            { "name": "user_id", "type": "uuid" },
            { "name": "nickname", "type": "text" },
        ]}]
    })).await;

    let (s, body) = rt.request_as(Method::GET, "/api/v1/apps/hr/collections/profile", &tok, None).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(
        body.as_array().map(Vec::len), Some(0),
        "with ownership dropped, the '.own' key grants nothing at all: {body}",
    );

    for (label, sql) in [
        ("policies", "SELECT count(*) FROM pg_policies WHERE schemaname = 'hr' AND policyname LIKE '%_own'"),
        ("keys", "SELECT count(*) FROM rootcx_system.rbac_permissions WHERE key LIKE 'app:hr:%.own'"),
        ("projection", "SELECT count(*) FROM rootcx_system.sensitive_fields WHERE app_id = 'hr' AND owner_field IS NOT NULL"),
    ] {
        let left: i64 = sqlx::query_scalar(sql).fetch_one(rt.pool()).await.unwrap();
        assert_eq!(left, 0, "redeploy without ownership must leave no {label} behind");
    }

    rt.shutdown().await;
}

/// An unusable owner declaration is refused *before any DDL runs*, so a rejected
/// install leaves nothing behind. Validation moved after the first
/// `CREATE SCHEMA` would still answer 400 and still look correct from the
/// outside, while stranding a half-built app — which only the database can show.
///
/// Which declarations are unusable, and what each error says, is
/// `manifest::an_owner_declaration_no_policy_can_use_is_refused`; one case here is
/// enough to prove the validator is wired to the endpoint.
#[tokio::test]
async fn a_refused_install_leaves_no_schema_behind() {
    let rt = harness::TestRuntime::boot().await;

    let (s, body) = rt.post_json("/api/v1/apps", &json!({
        "appId": "bad", "name": "bad", "version": "1.0.0",
        "dataContract": [{ "entityName": "thing", "fields": [
            { "name": "count", "type": "number", "owner": true },
        ]}],
    })).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.to_string().contains("must be entity_link, uuid or text"), "{body}");

    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM information_schema.schemata WHERE schema_name = 'bad')",
    ).fetch_one(rt.pool()).await.unwrap();
    assert!(!exists, "validation must run before any DDL");

    rt.shutdown().await;
}

/// The `.own` suffix means "narrower than the base key" to the delegation
/// lattice, so an app must not be able to declare it as an unrelated capability.
///
/// Reserved on the way *in* only. The same suffix has to stay grantable on a role,
/// or the core-minted keys would be refused by the very endpoint an operator uses
/// to hand them out — the feature would ship dead. Both directions asserted here
/// because one guard is a charset check and the other adds the reservation, and it
/// is the pairing that has to hold.
#[tokio::test]
async fn own_is_reserved_for_the_core_yet_still_grantable() {
    let rt = harness::TestRuntime::boot().await;
    install_owned(&rt).await;

    let (s, body) = rt.post_json("/api/v1/apps", &json!({
        "appId": "sneaky", "name": "sneaky", "version": "1.0.0",
        "permissions": { "permissions": [{ "key": "reports.export.own", "description": "export" }] },
    })).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "an app must not declare the suffix: {body}");
    assert!(body.to_string().contains("reserved"), "{body}");

    for (label, method, path, payload) in [
        ("create a role with it", Method::POST, "/api/v1/roles".to_string(),
         json!({"name": "own_reader", "permissions": ["app:hr:profile.read.own"]})),
        ("update a role to hold it", Method::PATCH, "/api/v1/roles/own_reader".to_string(),
         json!({"permissions": ["app:hr:profile.read.own", "app:hr:profile.update.own"]})),
    ] {
        let (s, body) = rt.request_as(method, &path, &rt.token, Some(&payload)).await;
        assert_eq!(s, StatusCode::OK, "{label}: a core-minted key must stay grantable: {body}");
    }

    rt.shutdown().await;
}

/// Adopting ownership on a table that already holds rows must need no backfill:
/// a NULL owner is simply nobody's, so it is invisible to a confined caller and
/// still fully visible to an unscoped one.
#[tokio::test]
async fn an_unowned_legacy_row_is_nobody_s() {
    let rt = harness::TestRuntime::boot().await;

    // Install without ownership, then create a row that predates the declaration.
    let fields = |owner: Value| json!({
        "appId": "hr", "name": "hr", "version": "1.0.0",
        "dataContract": [{ "entityName": "profile", "fields": [
            { "name": "user_id", "type": "uuid", "owner": owner },
            { "name": "nickname", "type": "text" },
        ]}]
    });
    rt.install_manifest(&fields(json!(false))).await;
    rt.create("hr", "profile", &json!({"nickname": "legacy"})).await;
    rt.install_manifest(&fields(json!(true))).await;

    let (scoped, _) = user_with(&rt, "jean@t.local", &["app:hr:profile.read.own"]).await;
    let (_, body) = rt.request_as(Method::GET, "/api/v1/apps/hr/collections/profile", &scoped, None).await;
    assert_eq!(body.as_array().map(Vec::len), Some(0), "an unowned row belongs to no one: {body}");

    let (_, body) = rt.get_json("/api/v1/apps/hr/collections/profile").await;
    assert_eq!(body.as_array().map(Vec::len), Some(1), "and is still there for an unscoped reader: {body}");

    rt.shutdown().await;
}
