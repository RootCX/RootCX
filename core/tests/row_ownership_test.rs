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

// ── Delegated ownership ─────────────────────────────────────────────────
//
// A row that carries no user id can still belong to someone: a submission is
// mine because its assignment is, and that assignment because its enrollment is.
// Two links, because at one link the chain builder and a single-hop special case
// are indistinguishable.

async fn install_chained(rt: &harness::TestRuntime) {
    let link = |target: &str| json!({
        "name": format!("{target}_id"), "type": "entity_link",
        "references": { "entity": target, "field": "id" }, "owner": true,
    });
    rt.install_manifest(&json!({
        "appId": "school", "name": "school", "version": "1.0.0",
        "dataContract": [
            { "entityName": "enrollment", "fields": [
                { "name": "user_id", "type": "uuid", "owner": true },
                { "name": "label", "type": "text" }]},
            { "entityName": "assignment", "fields": [
                link("enrollment"), { "name": "title", "type": "text" }]},
            { "entityName": "submission", "fields": [
                link("assignment"), { "name": "note", "type": "text" }]},
        ]
    })).await;
}

/// One enrollment, assignment and submission belonging to `owner`, created by the
/// admin so no fixture depends on the policies under test.
async fn stack_for(rt: &harness::TestRuntime, owner: Uuid, tag: &str) -> (String, String) {
    let id = |v: Value| v["id"].as_str().expect("created record returns its id").to_string();
    let enrollment = id(rt.create("school", "enrollment", &json!({"user_id": owner, "label": tag})).await);
    let assignment = id(rt.create("school", "assignment",
        &json!({"enrollment_id": enrollment, "title": tag})).await);
    let submission = id(rt.create("school", "submission",
        &json!({"assignment_id": assignment, "note": tag})).await);
    (assignment, submission)
}

/// The feature itself: `.own` on a table that holds no user id anywhere confines
/// its holder to the rows reachable from its own root, two links away — for reads
/// and, through `WITH CHECK`, for the re-parenting that would otherwise be the way
/// out of the scope.
#[tokio::test]
async fn own_follows_a_two_link_delegation_chain() {
    let rt = harness::TestRuntime::boot().await;
    install_chained(&rt).await;

    let scoped = ["read", "update", "delete", "create"]
        .map(|action| format!("app:school:submission.{action}.own"));
    let scoped: Vec<&str> = scoped.iter().map(String::as_str).collect();
    let (tok, mine) = user_with(&rt, "jean@t.local", &scoped).await;
    let (_, theirs) = user_with(&rt, "marie@t.local", &[]).await;
    let (my_assignment, my_submission) = stack_for(&rt, mine, "jean").await;
    let (their_assignment, their_submission) = stack_for(&rt, theirs, "marie").await;
    // A submission hanging off nothing: unowned all the way down.
    rt.create("school", "submission", &json!({"note": "orphan"})).await;

    let (s, body) = rt.request_as(Method::GET, "/api/v1/apps/school/collections/submission", &tok, None).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let rows = body.as_array().expect("list returns an array");
    assert_eq!(rows.len(), 1, "ownership must resolve through both links: {body}");
    assert_eq!(rows[0]["note"], json!("jean"));

    for (label, method, id, patch, expected) in [
        ("read another's submission", Method::GET, &their_submission, None, StatusCode::NOT_FOUND),
        ("edit its own", Method::PATCH, &my_submission, Some(json!({"note": "revised"})), StatusCode::OK),
        // Visible and writable, but the row would come to rest under a root that
        // is not the caller's — the only escape a confined UPDATE has left.
        ("re-parent its own away", Method::PATCH, &my_submission,
         Some(json!({"assignment_id": their_assignment})), StatusCode::FORBIDDEN),
        ("delete another's", Method::DELETE, &their_submission, None, StatusCode::NOT_FOUND),
    ] {
        let path = format!("/api/v1/apps/school/collections/submission/{id}");
        let (s, response) = rt.request_as(method, &path, &tok, patch.as_ref()).await;
        assert_eq!(s, expected, "{label}: {response}");
    }

    for (label, assignment, expected) in [
        ("under its own assignment", json!(my_assignment), StatusCode::CREATED),
        ("under another's assignment", json!(their_assignment), StatusCode::FORBIDDEN),
        // No parent at all is no different from a parent that is not the caller's.
        ("under no assignment", json!(null), StatusCode::FORBIDDEN),
    ] {
        let body = json!({"assignment_id": assignment, "note": "new"});
        let (s, response) = rt.request_as(
            Method::POST, "/api/v1/apps/school/collections/submission", &tok, Some(&body),
        ).await;
        assert_eq!(s, expected, "create {label}: {response}");
    }

    rt.shutdown().await;
}

/// The security property that makes delegation safe to grant: what a `.own` key
/// reaches is decided by the data alone. The chain is resolved by `SECURITY
/// DEFINER` functions precisely so it never consults the caller's grants on the
/// tables it crosses, and every permissive policy Postgres ORs in is gated on a key
/// this caller does not hold. So neither holding nothing on the parents nor holding
/// everything on them changes the answer.
#[tokio::test]
async fn no_grant_on_the_chain_widens_what_own_sees() {
    let rt = harness::TestRuntime::boot().await;
    install_chained(&rt).await;

    let (blind, mine) = user_with(&rt, "jean@t.local", &["app:school:submission.read.own"]).await;
    // The same scope, plus unscoped authority over every table the chain crosses.
    let (informed, also_mine) = user_with(&rt, "paul@t.local", &[
        "app:school:submission.read.own",
        "app:school:enrollment.read", "app:school:enrollment.update",
        "app:school:assignment.read", "app:school:assignment.update",
    ]).await;
    let (_, theirs) = user_with(&rt, "marie@t.local", &[]).await;
    for (owner, tag) in [(mine, "jean"), (also_mine, "paul"), (theirs, "marie")] {
        stack_for(&rt, owner, tag).await;
    }

    for (label, token, tag) in [
        ("no grant at all on the parents", &blind, "jean"),
        ("full unscoped authority over the parents", &informed, "paul"),
    ] {
        let (s, body) = rt.request_as(
            Method::GET, "/api/v1/apps/school/collections/submission", token, None,
        ).await;
        assert_eq!(s, StatusCode::OK, "{label}: {body}");
        let rows = body.as_array().expect("list returns an array");
        assert_eq!(rows.len(), 1, "{label}: the scope is the same either way: {body}");
        assert_eq!(rows[0]["note"], json!(tag), "{label}: and it is the caller's own row");
    }

    rt.shutdown().await;
}

/// What the generated SQL is, and what it costs.
///
/// The shape is asserted against the catalog because it is the security boundary:
/// each link must be crossed through the `SECURITY DEFINER` resolver, never by an
/// inline subquery — which Postgres would run under the *caller's* policies on the
/// parent table, coupling ownership to grants and making a longer chain recurse
/// until the table is unreadable. The cost is asserted against the planner, because
/// `= ANY (ARRAY(...))` and the equivalent `IN (...)` are indistinguishable by
/// reading and differ by a whole sequential scan.
#[tokio::test]
async fn each_link_is_crossed_by_an_indexable_resolver() {
    let rt = harness::TestRuntime::boot().await;
    install_chained(&rt).await;

    let qual: String = sqlx::query_scalar(
        "SELECT qual FROM pg_policies WHERE schemaname = 'school' AND tablename = 'submission' \
           AND policyname = 'rootcx_rls_select_own'",
    ).fetch_one(rt.pool()).await.unwrap();
    assert!(
        qual.contains("assignment_id = ANY (ARRAY( SELECT rootcx_system.\"rootcx_own.school.assignment\"()"),
        "the policy must reach its parent through the resolver, as an array: {qual}",
    );

    // And the middle resolver must in turn defer to the root's, rather than
    // re-deriving ownership itself.
    let body: String = sqlx::query_scalar(
        "SELECT pg_get_functiondef(p.oid) FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace \
          WHERE n.nspname = 'rootcx_system' AND p.proname = 'rootcx_own.school.assignment'",
    ).fetch_one(rt.pool()).await.unwrap();
    assert!(body.contains("rootcx_own.school.enrollment"), "the chain must compose: {body}");
    assert!(body.contains("SECURITY DEFINER"), "a link crossed under RLS would recurse: {body}");

    let (_, mine) = user_with(&rt, "jean@t.local", &["app:school:submission.read.own"]).await;
    stack_for(&rt, mine, "jean").await;
    // Everyone else's work, so the caller's own rows are the needle they are in
    // production. A table where every row belongs to the caller has no plan worth
    // asserting on: a sequential scan is then the correct choice.
    for sql in [
        "INSERT INTO school.enrollment (user_id) SELECT gen_random_uuid() FROM generate_series(1, 2000)",
        "INSERT INTO school.assignment (enrollment_id) SELECT id FROM school.enrollment",
        "INSERT INTO school.submission (assignment_id) SELECT id FROM school.assignment",
        "ANALYZE school.submission",
    ] {
        sqlx::query(sql).execute(rt.pool()).await.unwrap();
    }
    let plan: Vec<String> = sqlx::query_scalar(
        "EXPLAIN SELECT * FROM school.submission \
          WHERE assignment_id = ANY (ARRAY(SELECT rootcx_system.\"rootcx_own.school.assignment\"()))",
    ).fetch_all(rt.pool()).await.unwrap();
    let plan = plan.join("\n");
    assert!(
        plan.contains("Index Scan") || plan.contains("Bitmap Index Scan"),
        "a confined read must ride the link's index; plan was:\n{plan}",
    );

    rt.shutdown().await;
}

/// A delegation that cannot terminate is refused at install, with the manifest
/// mistake named. Left to the database, a loop is reported by Postgres as
/// `infinite recursion detected in policy` only once the table is queried — after
/// the deploy has been declared a success, and with the table unreadable until the
/// manifest is fixed. A chain ending on an entity that owns nothing is quieter
/// still: the policies simply match no row, which reads as an access bug.
#[tokio::test]
async fn a_delegation_that_cannot_terminate_is_refused() {
    let rt = harness::TestRuntime::boot().await;

    let link = |target: &str| json!({
        "name": format!("{target}_id"), "type": "entity_link",
        "references": { "entity": target, "field": "id" }, "owner": true,
    });
    for (label, needle, entities) in [
        ("a chain ending on an entity that owns nothing", "declares no owner field", json!([
            { "entityName": "ticket", "fields": [{ "name": "subject", "type": "text" }]},
            { "entityName": "comment", "fields": [link("ticket")]},
        ])),
        ("two entities each owned by the other", "in a loop", json!([
            { "entityName": "ticket", "fields": [link("comment")]},
            { "entityName": "comment", "fields": [link("ticket")]},
        ])),
    ] {
        let (s, body) = rt.post_json("/api/v1/apps", &json!({
            "appId": "helpdesk", "name": "helpdesk", "version": "1.0.0", "dataContract": entities,
        })).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{label}: {body}");
        assert!(body.to_string().contains(needle), "{label}: {body}");

        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM information_schema.schemata WHERE schema_name = 'helpdesk')",
        ).fetch_one(rt.pool()).await.unwrap();
        assert!(!exists, "{label}: validation must run before any DDL");
    }

    rt.shutdown().await;
}

/// The resolvers live in `rootcx_system`, so neither a redeploy that drops the
/// delegation nor dropping the app takes them with it. Both must reconcile: a
/// resolver left behind is a callable description of who owns what in a schema that
/// may since have been reshaped, or removed entirely.
#[tokio::test]
async fn resolvers_track_the_declaration() {
    let rt = harness::TestRuntime::boot().await;
    install_chained(&rt).await;

    let resolvers = async || {
        sqlx::query_scalar::<_, String>(
            "SELECT p.proname FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace \
              WHERE n.nspname = 'rootcx_system' AND p.proname LIKE 'rootcx\\_own.%' ORDER BY 1",
        ).fetch_all(rt.pool()).await.unwrap()
    };
    assert_eq!(
        resolvers().await,
        ["rootcx_own.school.assignment", "rootcx_own.school.enrollment"],
        "one resolver per entity another entity defers to, and none for the leaf",
    );

    // Redeploy with the last link dropped: `assignment` is nobody's parent now.
    rt.install_manifest(&json!({
        "appId": "school", "name": "school", "version": "1.0.1",
        "dataContract": [
            { "entityName": "enrollment", "fields": [
                { "name": "user_id", "type": "uuid", "owner": true }]},
            { "entityName": "assignment", "fields": [
                { "name": "enrollment_id", "type": "entity_link",
                  "references": { "entity": "enrollment", "field": "id" }, "owner": true }]},
            { "entityName": "submission", "fields": [
                { "name": "assignment_id", "type": "entity_link",
                  "references": { "entity": "assignment", "field": "id" }}]},
        ]
    })).await;
    assert_eq!(
        resolvers().await, ["rootcx_own.school.enrollment"],
        "a redeploy that stops delegating must leave no resolver behind",
    );
    let left: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_policies WHERE schemaname = 'school' \
           AND tablename = 'submission' AND policyname LIKE '%\\_own'",
    ).fetch_one(rt.pool()).await.unwrap();
    assert_eq!(left, 0, "nor any policy confining a table that claims no owner");

    assert_eq!(rt.delete("/api/v1/apps/school").await, StatusCode::OK);
    assert!(resolvers().await.is_empty(), "uninstall must take the resolvers with it");

    rt.shutdown().await;
}

/// The three catalog properties every resolver's safety rests on, asserted where
/// they are actually recorded rather than where they are written.
///
/// `STABLE` is the load-bearing one, and the easiest to "optimise" away: the answer
/// depends on the caller's GUCs, so an `IMMUTABLE` marking would let the planner
/// constant-fold one caller's reachable set into a cached plan and hand it to every
/// other user — a permanent cross-user leak, invisible until two users share a
/// backend. `SECURITY DEFINER` is what makes ownership a fact about the data instead
/// of about the caller's grants on the tables the chain crosses. And a function is
/// executable by PUBLIC unless revoked, while this one names a specific user's rows.
#[tokio::test]
async fn resolvers_are_stable_secdef_and_not_public() {
    let rt = harness::TestRuntime::boot().await;
    install_chained(&rt).await;

    // grantee 0 is PUBLIC; a NULL acl means the default, which *is* PUBLIC EXECUTE.
    let resolvers: Vec<(String, String, bool, bool)> = sqlx::query_as(
        "SELECT p.proname, p.provolatile::text, p.prosecdef, \
                p.proacl IS NULL OR EXISTS (SELECT 1 FROM aclexplode(p.proacl) a \
                    WHERE a.grantee = 0 AND a.privilege_type = 'EXECUTE') \
           FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace \
          WHERE n.nspname = 'rootcx_system' AND p.proname LIKE 'rootcx\\_own.%' ORDER BY 1",
    ).fetch_all(rt.pool()).await.unwrap();

    assert_eq!(resolvers.len(), 2, "the fixture must have produced resolvers to assert on: {resolvers:?}");
    for (name, volatility, secdef, public_execute) in resolvers {
        assert_eq!(volatility, "s", "{name} must be STABLE, never IMMUTABLE: a cached plan would leak one caller's rows to another");
        assert!(secdef, "{name} must be SECURITY DEFINER, or ownership would depend on the caller's grants");
        assert!(!public_execute, "{name} enumerates a user's rows and must not be executable by PUBLIC");
    }

    rt.shutdown().await;
}

/// A resolver bypasses RLS by design, and RLS predicates run as the *invoking*
/// role, so `rootcx_app_executor` must hold EXECUTE on it — and every app's
/// `ctx.sql` runs as that same role. Apps being mutually untrusted, an app must not
/// be able to call another app's resolver and enumerate the caller's row ids there.
///
/// The resolver therefore also checks `rootcx.app_id`, posed by `set_rls_context`
/// before the drop to the executor. `set_config` is revoked from that role, so an
/// app cannot claim to be another one. Both directions are asserted, because a
/// guard that denies everything would pass the first half on its own.
#[tokio::test]
async fn one_app_cannot_resolve_another_s_ownership() {
    let rt = harness::TestRuntime::boot().await;
    install_chained(&rt).await;
    rt.install("crm", "contacts").await;

    let (_, mine) = user_with(&rt, "jean@t.local", &["app:school:submission.read.own"]).await;
    stack_for(&rt, mine, "jean").await;
    let context = rootcx_core::governance::enforcement::ContextState {
        user_id: Some(mine),
        is_delegated: false,
        effective_perms: vec![],
        connection_id: None,
        audit_actor_id: Some(mine),
        audit_delegator_id: None,
    };

    let probe = "SELECT rootcx_system.\"rootcx_own.school.assignment\"() AS id";
    for (label, app, expected) in [
        ("its own app", "school", 1),
        ("a bystander app", "crm", 0),
    ] {
        let ok = rootcx_core::governance::enforcement::run_sql(rt.pool(), app, &context, probe, &[])
            .await
            .unwrap_or_else(|e| panic!("{label}: the call itself must succeed, not error: {e}"));
        assert_eq!(ok.row_count, expected, "{label}: resolver answered the wrong caller");
    }

    // And the guard confines the resolver, not the feature: the same caller's
    // confined read through its own app still returns its row.
    let ok = rootcx_core::governance::enforcement::run_sql(
        rt.pool(), "school", &context, "SELECT note FROM school.submission", &[],
    ).await.unwrap();
    assert_eq!(ok.row_count, 1, "the app's own confined read must be unaffected: {ok:?}");

    rt.shutdown().await;
}

/// The boot pass rebuilds policies from the `sensitive_fields` projection, not from
/// the manifest — a path structurally different from install, and the one every
/// existing tenant takes when it first boots on a core that has this feature. No
/// install-path test can reach it.
///
/// Delegation is what makes that path non-trivial: resolving one entity needs the
/// projection rows of the entities it defers to, so the pass has to group per schema
/// rather than handle each table on its own. Get that wrong and the table simply
/// ends up with no row-scoped policies — `.own` holders are denied, silently, and it
/// reads as an access bug rather than a boot bug.
///
/// So this asserts the rebuilt *behavior*, not the presence of catalog rows: a
/// predicate rebuilt into the wrong shape would satisfy a policy count and still
/// confine the wrong caller.
#[tokio::test]
async fn the_boot_pass_rebuilds_a_delegated_chain_from_the_projection() {
    let rt = harness::TestRuntime::boot().await;
    install_chained(&rt).await;

    let (tok, mine) = user_with(&rt, "jean@t.local", &["app:school:submission.read.own"]).await;
    stack_for(&rt, mine, "jean").await;
    let (_, theirs) = user_with(&rt, "marie@t.local", &[]).await;
    stack_for(&rt, theirs, "marie").await;

    // Put the tenant in the state it is in before the pass runs: the projection is
    // there (it is written at deploy and survives), the artifacts derived from it
    // are not. Policies first — a resolver cannot be dropped while one names it.
    let policies: Vec<(String, String)> = sqlx::query_as(
        "SELECT tablename, policyname FROM pg_policies \
          WHERE schemaname = 'school' AND policyname LIKE '%\\_own'",
    ).fetch_all(rt.pool()).await.unwrap();
    assert_eq!(policies.len(), 12, "three owned entities, four commands each");
    for (table, policy) in policies {
        sqlx::query(&format!("DROP POLICY {policy} ON school.{table}"))
            .execute(rt.pool()).await.unwrap();
    }
    for entity in ["assignment", "enrollment"] {
        sqlx::query(&format!("DROP FUNCTION rootcx_system.\"rootcx_own.school.{entity}\"()"))
            .execute(rt.pool()).await.unwrap();
    }

    use rootcx_core::extensions::RuntimeExtension;
    rootcx_core::extensions::rbac::RbacExtension.bootstrap(rt.pool()).await
        .expect("the boot pass must survive a schema whose ownership is delegated");

    let rebuilt: Vec<String> = sqlx::query_scalar(
        "SELECT p.proname FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace \
          WHERE n.nspname = 'rootcx_system' AND p.proname LIKE 'rootcx\\_own.%' ORDER BY 1",
    ).fetch_all(rt.pool()).await.unwrap();
    assert_eq!(
        rebuilt, ["rootcx_own.school.assignment", "rootcx_own.school.enrollment"],
        "the chain must be reconstructible from the projection alone",
    );

    let (s, body) = rt.request_as(
        Method::GET, "/api/v1/apps/school/collections/submission", &tok, None,
    ).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let rows = body.as_array().expect("list returns an array");
    assert_eq!(rows.len(), 1, "the rebuilt policy must confine, not deny or open: {body}");
    assert_eq!(rows[0]["note"], json!("jean"), "and confine to the caller's own root");

    rt.shutdown().await;
}
