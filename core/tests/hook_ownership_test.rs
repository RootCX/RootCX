//! Entity hooks vs row ownership — what a hook's payload is allowed to carry.
//!
//! A hook fires as its registrant and receives the whole written row, so its
//! payload is a read of the watched table performed on that registrant's behalf.
//! The trigger is `SECURITY DEFINER` and runs before any policy, so nothing but
//! its own gate confines it — which is why these tests exist at all.
//!
//! Every case fires the trigger inside a transaction that is then rolled back, and
//! counts what landed in `pgmq.q_jobs` from within that same transaction. The
//! scheduler is running in-process and deletes a job the moment it fails to
//! dispatch it, so a committed write gives a racy answer; an uncommitted one is
//! invisible to it and exact for us.

mod harness;

use serde_json::{Value, json};
use uuid::Uuid;

/// `profile` is owned directly, through a `uuid` column, and also declares a
/// sensitive column — the payload must keep stripping it whatever the gate decides.
async fn install_owned(rt: &harness::TestRuntime) {
    rt.install_manifest(&json!({
        "appId": "hr", "name": "hr", "version": "1.0.0",
        "dataContract": [{ "entityName": "profile", "fields": [
            { "name": "user_id", "type": "uuid", "owner": true },
            { "name": "nickname", "type": "text" },
            { "name": "api_key", "type": "text", "sensitive": true },
        ]}]
    })).await;
}

/// Ownership two links from any user id, so the delegated case goes through the
/// `SECURITY DEFINER` resolvers rather than a comparison.
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

/// A user holding exactly `perms` — no inherited role, no admin.
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

/// Register an UPDATE hook as `token`'s user, the way a user actually would.
async fn hook_on(rt: &harness::TestRuntime, app: &str, entity: &str, token: &str) -> String {
    let (status, body) = rt.request_as(
        reqwest::Method::POST, &format!("/api/v1/apps/{app}/hooks"), token,
        Some(&json!({ "entity": entity, "operation": "UPDATE", "action_type": "job" })),
    ).await;
    assert!(status.is_success(), "hook registration failed: {body}");
    body["id"].as_str().expect("a created hook returns its id").to_string()
}

/// Fire the UPDATE trigger on one row and return what it enqueued, then undo the
/// write. `set` is applied verbatim, so a test can also move a row's owner.
async fn fire_update(rt: &harness::TestRuntime, table: &str, id: &str, set: &str) -> Vec<Value> {
    let mut tx = rt.pool().begin().await.unwrap();
    let touched = sqlx::query(&format!("UPDATE {table} SET {set} WHERE id = $1::uuid"))
        .bind(id).execute(&mut *tx).await
        .expect("the actor's write must survive whatever the hook gate decides")
        .rows_affected();
    assert_eq!(touched, 1, "fixture row {id} not found in {table}");

    let messages: Vec<(Value,)> = sqlx::query_as(
        "SELECT message FROM pgmq.q_jobs WHERE message->'payload'->>'record_id' = $1",
    ).bind(id).fetch_all(&mut *tx).await.unwrap();
    tx.rollback().await.unwrap();
    messages.into_iter().map(|(m,)| m).collect()
}

/// What `hook` received out of one fire. Filtered by hook id, so several
/// registrants can share a table without reading each other's results.
fn received<'m>(messages: &'m [Value], hook: &str) -> Vec<&'m Value> {
    messages.iter()
        .filter(|m| m["payload"]["hook_id"].as_str() == Some(hook))
        .collect()
}

fn id_of(row: Value) -> String {
    row["id"].as_str().expect("a created record returns its id").to_string()
}

/// One enrollment, assignment and submission belonging to `owner`, created by the
/// admin so no fixture depends on the confinement under test.
async fn submission_for(rt: &harness::TestRuntime, owner: Uuid, tag: &str) -> String {
    let enrollment = id_of(rt.create("school", "enrollment", &json!({"user_id": owner, "label": tag})).await);
    let assignment = id_of(rt.create("school", "assignment",
        &json!({"enrollment_id": enrollment, "title": tag})).await);
    id_of(rt.create("school", "submission", &json!({"assignment_id": assignment, "note": tag})).await)
}

/// The vulnerability, and its fix, on a directly-owned entity.
///
/// `hook.write` is minted for every app, so registering a hook is self-service.
/// Before the gate, that alone turned any writer's row into the registrant's
/// payload; the whole point is that the payload now stops exactly where a SELECT
/// by the same registrant would.
#[tokio::test]
async fn a_hook_receives_only_the_rows_its_registrant_could_read() {
    let rt = harness::TestRuntime::boot().await;
    install_owned(&rt).await;

    let (unscoped, _) = user_with(&rt, "boss@t.local",
        &["app:hr:hook.write", "app:hr:profile.read"]).await;
    let (scoped, mine) = user_with(&rt, "jean@t.local",
        &["app:hr:hook.write", "app:hr:profile.read.own"]).await;
    let (blind, _) = user_with(&rt, "eve@t.local", &["app:hr:hook.write"]).await;
    let (_, theirs) = user_with(&rt, "marie@t.local", &[]).await;

    let unscoped_hook = hook_on(&rt, "hr", "profile", &unscoped).await;
    let scoped_hook = hook_on(&rt, "hr", "profile", &scoped).await;
    let blind_hook = hook_on(&rt, "hr", "profile", &blind).await;

    let own_row = id_of(rt.create("hr", "profile",
        &json!({"user_id": mine, "nickname": "jean", "api_key": "s3cret"})).await);
    let other_row = id_of(rt.create("hr", "profile",
        &json!({"user_id": theirs, "nickname": "marie", "api_key": "s3cret"})).await);

    for (label, row, scoped_gets) in [
        ("its registrant's own row", &own_row, 1),
        ("another owner's row", &other_row, 0),
    ] {
        let fired = fire_update(&rt, "hr.profile", row, "nickname = nickname || '!'").await;
        assert_eq!(received(&fired, &unscoped_hook).len(), 1,
            "{label}: an unscoped registrant keeps receiving everything: {fired:?}");
        assert_eq!(received(&fired, &scoped_hook).len(), scoped_gets,
            "{label}: a '.own' registrant receives it only when it owns it: {fired:?}");
        assert!(received(&fired, &blind_hook).is_empty(),
            "{label}: a registrant who can read the table neither way receives nothing: {fired:?}");
    }

    // Payload shape, on the path that is meant to be unchanged.
    let fired = fire_update(&rt, "hr.profile", &own_row, "nickname = 'renamed'").await;
    let payload = &received(&fired, &unscoped_hook)[0]["payload"];
    assert_eq!(payload["record"]["nickname"], json!("renamed"), "the new row is there: {payload}");
    assert_eq!(payload["old_record"]["nickname"], json!("jean"), "and the old one: {payload}");
    for side in ["record", "old_record"] {
        assert!(payload[side].get("api_key").is_none(),
            "a sensitive column stays stripped: {payload}");
        assert_eq!(payload[side]["user_id"], json!(mine.to_string()),
            "the owning column is not itself stripped: {payload}");
    }

    // Ownership of an UPDATE is both sides of it: `old_record` carries the row as
    // it was, so receiving a row on its way out is receiving it.
    let fired = fire_update(&rt, "hr.profile", &own_row,
        &format!("user_id = '{theirs}'::uuid")).await;
    assert!(received(&fired, &scoped_hook).is_empty(),
        "a row handed away must not be delivered to the owner it left: {fired:?}");

    rt.shutdown().await;
}

/// Fail-closed on a projection that outlived its column.
///
/// The projection is written at deploy and never revalidated, so "the column named
/// there is gone" is reachable in production. It must cost the hook, never the
/// write: the trigger runs inside a stranger's transaction, and an exception there
/// would refuse an INSERT for a hook that stranger never registered.
#[tokio::test]
async fn a_stale_ownership_projection_costs_the_hook_and_not_the_write() {
    let rt = harness::TestRuntime::boot().await;
    install_owned(&rt).await;

    let (unscoped, _) = user_with(&rt, "boss@t.local",
        &["app:hr:hook.write", "app:hr:profile.read"]).await;
    let (scoped, mine) = user_with(&rt, "jean@t.local",
        &["app:hr:hook.write", "app:hr:profile.read.own"]).await;
    let unscoped_hook = hook_on(&rt, "hr", "profile", &unscoped).await;
    let scoped_hook = hook_on(&rt, "hr", "profile", &scoped).await;
    let row = id_of(rt.create("hr", "profile", &json!({"user_id": mine, "nickname": "jean"})).await);

    sqlx::query(
        "UPDATE rootcx_system.sensitive_fields SET owner_field = 'column_that_was_dropped' \
         WHERE app_id = 'hr' AND entity = 'profile'",
    ).execute(rt.pool()).await.unwrap();

    // The write itself is the first assertion: `fire_update` panics if the UPDATE
    // raises, and the row must still be updated.
    let fired = fire_update(&rt, "hr.profile", &row, "nickname = 'jeannot'").await;
    assert!(received(&fired, &scoped_hook).is_empty(),
        "unresolvable ownership must deny, not over-share: {fired:?}");
    assert_eq!(received(&fired, &unscoped_hook).len(), 1,
        "and the failure stays on the ownership path: {fired:?}");

    rt.shutdown().await;
}

/// Delegated ownership: the row holds no user id anywhere, and the answer comes
/// from the resolver the RLS policy installed — not from a second chain walk in
/// PL/pgSQL, which is the version that would eventually disagree with the policy.
///
/// Then the resolver is dropped, which is how a missing one raises
/// `undefined_function` inside the actor's write.
#[tokio::test]
async fn a_hook_crosses_a_delegation_chain_and_survives_a_missing_resolver() {
    let rt = harness::TestRuntime::boot().await;
    install_chained(&rt).await;

    let (scoped, mine) = user_with(&rt, "jean@t.local",
        &["app:school:hook.write", "app:school:submission.read.own"]).await;
    let (_, theirs) = user_with(&rt, "marie@t.local", &[]).await;
    let hook = hook_on(&rt, "school", "submission", &scoped).await;

    let own_row = submission_for(&rt, mine, "jean").await;
    let other_row = submission_for(&rt, theirs, "marie").await;

    for (label, row, expected) in [
        ("owned two links away", &own_row, 1),
        ("owned by someone else", &other_row, 0),
    ] {
        let fired = fire_update(&rt, "school.submission", row, "note = note || '!'").await;
        assert_eq!(received(&fired, &hook).len(), expected,
            "{label}: the chain decides the payload: {fired:?}");
    }

    // Point the projection at an entity that has no resolver, which is what a
    // dropped one looks like from inside the trigger. The function itself cannot be
    // dropped while the RLS policy naming it stands, and this is the same failure.
    sqlx::query(
        "UPDATE rootcx_system.sensitive_fields SET owner_parent = 'entity_without_a_resolver' \
         WHERE app_id = 'school' AND entity = 'submission'",
    ).execute(rt.pool()).await.unwrap();
    let fired = fire_update(&rt, "school.submission", &own_row, "note = 'orphaned'").await;
    assert!(received(&fired, &hook).is_empty(),
        "a missing resolver denies the hook without aborting the write: {fired:?}");

    rt.shutdown().await;
}
