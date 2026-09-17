//! Assignment sharing across the real install -> RLS -> Bun SQL boundaries.
//!
//! Mutation targets (see scripts/row-access-mutations.ts for executable checks):
//! - Remove grantee ownership or reverse grantee/subject: exact many-to-many IDs fail.
//! - Return only the assigned enrollment rather than its Core identity: sibling
//!   enrollment / second-person IDs fail.
//! - Remove activeWhen, or interpret end_date as an expiry: revocation cases fail.
//! - Cache shared keys across statements / use REPEATABLE READ: callback race fails.
//! - Remove the exact entity.read.shared gate: adjacent-entity resolver probe fails.
//! - Remove app/user guards: foreign/missing-context resolver probes fail.
//! - OR shared access into INSERT/UPDATE/DELETE or omit ownership WITH CHECK:
//!   denied mutations or the own reparenting case change the persisted snapshot.
//! - Skip share cleanup/reconstruction: lifecycle reads and artifact checks fail.
//! - Correlate the resolver per target row / replace the hashed membership test:
//!   restricted-role EXPLAIN exposes repeated work or a linear membership test.
//!
//! Compile the governance target first for each mutant, then run only this module
//! through the documented isolated test harness. A compile failure, boot failure
//! or timeout is not mutation evidence; record the failed security assertion.

use crate::harness::{self, TestRuntime};
use reqwest::{Method, StatusCode};
use rootcx_core::extensions::{RuntimeExtension, rbac::RbacExtension};
use serde_json::{Value, json};
use std::{collections::BTreeSet, time::Duration};
use uuid::Uuid;

const APP: &str = "assignment_care";
const OWNED: [&str; 4] = ["person", "enrollment", "profile", "note"];
const TABLES: [&str; 5] = ["person", "enrollment", "profile", "note", "assignment"];
const BARRIER: i64 = 846_261;

const BACKEND: &[u8] = br#"
serve({ rpc: {
  sql: async (params, _caller, ctx) => {
    try {
      const run = async (db) => db.sql(params.sql, params.args ?? []);
      return { result: params.transaction ? await ctx.transaction(run) : await run(ctx) };
    } catch (error) { return { error: error.message }; }
  },
  revocation: async (params, _caller, ctx) => {
    return ctx.transaction(async (tx) => {
      const before = await tx.sql(params.sql);
      const identity = await tx.sql(
        "SELECT pg_backend_pid(), current_setting('transaction_isolation'), current_user::text"
      );
      await tx.sql("SELECT pg_advisory_xact_lock($1::bigint)", [params.barrier]);
      const after = await tx.sql(params.sql);
      const finalIdentity = await tx.sql("SELECT pg_backend_pid()");
      return { before, after, identity, finalIdentity };
    });
  },
} });
"#;

fn manifest() -> Value {
    let owner_link = |name: &str, target: &str| {
        json!({
            "name": name, "type": "entity_link", "owner": true,
            "references": {"entity": target, "field": "id"}
        })
    };
    json!({
        "appId": APP, "name": "Assignment care", "version": "1.0.0",
        "dataContract": [
            {"entityName": "person", "fields": [
                owner_link("core_user_id", "core:users"),
                {"name": "label", "type": "text"}
            ]},
            {"entityName": "enrollment", "fields": [
                owner_link("person_id", "person"), {"name": "label", "type": "text"}
            ]},
            {"entityName": "profile", "fields": [
                owner_link("person_id", "person"), {"name": "label", "type": "text"}
            ]},
            {"entityName": "note", "fields": [
                owner_link("enrollment_id", "enrollment"), {"name": "label", "type": "text"}
            ]},
            {"entityName": "assignment", "share": {
                "grantee": "helper_enrollment_id", "subject": "helped_enrollment_id",
                "activeWhen": {"isNull": "end_date"}
            }, "fields": [
                {"name": "helper_enrollment_id", "type": "entity_link",
                 "references": {"entity": "enrollment", "field": "id"}},
                {"name": "helped_enrollment_id", "type": "entity_link",
                 "references": {"entity": "enrollment", "field": "id"}},
                {"name": "end_date", "type": "date"}
            ]}
        ]
    })
}

struct Person {
    user: Uuid,
    person: Uuid,
    enrollments: Vec<Uuid>,
    profile: Uuid,
    notes: Vec<Uuid>,
}

impl Person {
    fn ids(&self, entity: &str) -> Vec<Uuid> {
        match entity {
            "person" => vec![self.person],
            "enrollment" => self.enrollments.clone(),
            "profile" => vec![self.profile],
            "note" => self.notes.clone(),
            _ => panic!("no owned fixture rows for {entity}"),
        }
    }
}

struct Fixture {
    rt: TestRuntime,
    helpers: Vec<(String, Person)>,
    subjects: Vec<Person>,
    alias: Person,
    outsider: Person,
}

async fn user(rt: &TestRuntime, name: &str) -> (String, Uuid) {
    let email = format!("{name}@assignment.test");
    let token = rt.register_and_login(&email).await;
    let id: Uuid = sqlx::query_scalar("SELECT id FROM rootcx_system.users WHERE email = $1")
        .bind(email)
        .fetch_one(rt.pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM rootcx_system.rbac_assignments WHERE user_id = $1")
        .bind(id)
        .execute(rt.pool())
        .await
        .unwrap();
    let role = format!("assignment_{}", id.simple());
    sqlx::query("INSERT INTO rootcx_system.rbac_roles (name, permissions) VALUES ($1, '{}')")
        .bind(&role)
        .execute(rt.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO rootcx_system.rbac_assignments (user_id, role) VALUES ($1, $2)")
        .bind(id)
        .bind(role)
        .execute(rt.pool())
        .await
        .unwrap();
    (token, id)
}

async fn permissions(rt: &TestRuntime, user: Uuid, keys: Vec<String>) {
    sqlx::query("UPDATE rootcx_system.rbac_roles SET permissions = $1 WHERE name = $2")
        .bind(keys)
        .bind(format!("assignment_{}", user.simple()))
        .execute(rt.pool())
        .await
        .unwrap();
}

fn read_keys(own: bool) -> Vec<String> {
    let mut keys = vec![format!("app:{APP}:invoke")];
    for entity in OWNED {
        keys.push(format!("app:{APP}:{entity}.read.shared"));
        if own {
            keys.push(format!("app:{APP}:{entity}.read.own"));
        }
    }
    keys
}

async fn insert(rt: &TestRuntime, entity: &str, payload: Value) -> Uuid {
    let row = rt.create(APP, entity, &payload).await;
    Uuid::parse_str(row["id"].as_str().unwrap()).unwrap()
}

async fn person(rt: &TestRuntime, user: Uuid, label: &str) -> Person {
    let person = insert(rt, "person", json!({"core_user_id": user, "label": label})).await;
    let profile = insert(rt, "profile", json!({"person_id": person, "label": label})).await;
    let mut enrollments = vec![];
    let mut notes = vec![];
    for i in 0..2 {
        let enrollment = insert(
            rt,
            "enrollment",
            json!({"person_id": person, "label": format!("{label}-{i}")}),
        )
        .await;
        notes.push(
            insert(
                rt,
                "note",
                json!({"enrollment_id": enrollment, "label": label}),
            )
            .await,
        );
        enrollments.push(enrollment);
    }
    Person {
        user,
        person,
        enrollments,
        profile,
        notes,
    }
}

async fn fixture() -> Fixture {
    let rt = TestRuntime::boot().await;
    rt.install_manifest(&manifest()).await;
    let mut helpers = vec![];
    let mut subjects = vec![];
    for label in ["helper_a", "helper_b", "subject_a", "subject_b"] {
        let (token, uid) = user(&rt, label).await;
        let p = person(&rt, uid, label).await;
        if label.starts_with("helper") {
            permissions(&rt, uid, read_keys(true)).await;
            helpers.push((token, p));
        } else {
            subjects.push(p);
        }
    }
    // A second person with the same Core user catches implementations that share
    // only the linked person, rather than deriving the Core identity.
    let alias = person(&rt, subjects[0].user, "subject_a_alias").await;
    let (_, outsider_user) = user(&rt, "outsider").await;
    let outsider = person(&rt, outsider_user, "outsider").await;
    let (status, body) = rt
        .deploy(APP, &harness::make_tar_gz(&[("index.ts", BACKEND)]))
        .await;
    assert_eq!(status, StatusCode::OK, "fixture worker deploy: {body}");
    Fixture {
        rt,
        helpers,
        subjects,
        alias,
        outsider,
    }
}

async fn assign(rt: &TestRuntime, helper: Uuid, subject: Uuid, end: Option<&str>) -> Uuid {
    insert(
        rt,
        "assignment",
        json!({
            "helper_enrollment_id": helper, "helped_enrollment_id": subject, "end_date": end
        }),
    )
    .await
}

async fn rpc(
    rt: &TestRuntime,
    app: &str,
    token: &str,
    sql: &str,
    args: Value,
    transaction: bool,
) -> (StatusCode, Value) {
    rt.request_as(
        Method::POST,
        &format!("/api/v1/apps/{app}/rpc"),
        token,
        Some(&json!({"method": "sql", "params": {
            "sql": sql, "args": args, "transaction": transaction
        }})),
    )
    .await
}

fn select_ids(entity: &str) -> String {
    format!("SELECT id::text FROM {APP}.{entity} ORDER BY id")
}

fn row_ids(result: &Value) -> Vec<String> {
    result["rows"]
        .as_array()
        .unwrap_or_else(|| panic!("expected SQL rows: {result}"))
        .iter()
        .map(|r| r[0].as_str().expect("text ID").to_owned())
        .collect()
}

fn expected_ids(people: &[&Person], entity: &str) -> Vec<String> {
    people
        .iter()
        .flat_map(|p| p.ids(entity))
        .map(|id| id.to_string())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

async fn snapshot(rt: &TestRuntime) -> Value {
    let mut result = serde_json::Map::new();
    for entity in TABLES {
        let rows: Value = sqlx::query_scalar(&format!(
            "SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY id), '[]'::jsonb) FROM {APP}.{entity} t"
        ))
        .fetch_one(rt.pool())
        .await
        .unwrap();
        result.insert(entity.into(), rows);
    }
    Value::Object(result)
}

#[tokio::test]
async fn sharing_is_an_exact_many_to_many_core_identity_read_union() {
    let f = fixture().await;
    let h1 = &f.helpers[0].1;
    let h2 = &f.helpers[1].1;
    let s1 = &f.subjects[0];
    let s2 = &f.subjects[1];
    // Nullable ownership links represent nobody, not an invitation to share
    // all legacy or incomplete records.
    insert(&f.rt, "person", json!({"label": "unowned"})).await;
    let orphan = insert(&f.rt, "enrollment", json!({"label": "unowned"})).await;
    insert(&f.rt, "profile", json!({"label": "unowned"})).await;
    insert(
        &f.rt,
        "note",
        json!({"enrollment_id": orphan, "label": "unowned"}),
    )
    .await;
    insert(
        &f.rt,
        "assignment",
        json!({"helper_enrollment_id": h1.enrollments[0]}),
    )
    .await;
    insert(
        &f.rt,
        "assignment",
        json!({"helped_enrollment_id": s1.enrollments[0]}),
    )
    .await;
    assign(&f.rt, h1.enrollments[0], orphan, None).await;
    for (helper, subject) in [
        (h1.enrollments[0], s1.enrollments[0]),
        (h1.enrollments[1], s1.enrollments[1]), // duplicate identity
        (h1.enrollments[0], s2.enrollments[0]),
        (h2.enrollments[1], s1.enrollments[1]), // second helper
        (s1.enrollments[0], f.outsider.enrollments[0]), // never transitive
    ] {
        assign(&f.rt, helper, subject, None).await;
    }
    // Inactive edges must not contribute even when their end is far in the future.
    assign(
        &f.rt,
        h2.enrollments[0],
        s2.enrollments[0],
        Some("9999-12-31"),
    )
    .await;
    for (i, (token, helper)) in f.helpers.iter().enumerate() {
        let people = if i == 0 {
            vec![helper, s1, s2, &f.alias]
        } else {
            vec![helper, s1, &f.alias]
        };
        for entity in OWNED {
            let expected = expected_ids(&people, entity);
            for transaction in [false, true] {
                let (status, body) = rpc(
                    &f.rt,
                    APP,
                    token,
                    &select_ids(entity),
                    json!([]),
                    transaction,
                )
                .await;
                assert_eq!(
                    status,
                    StatusCode::OK,
                    "helper {i}/{entity}/tx={transaction}: {body}"
                );
                assert_eq!(
                    row_ids(&body["result"]),
                    expected,
                    "helper {i}/{entity}/tx={transaction}: {body}"
                );
            }
            let (status, body) =
                f.rt.request_as(
                    Method::GET,
                    &format!("/api/v1/apps/{APP}/collections/{entity}"),
                    token,
                    None,
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{entity}: {body}");
            let mut ids: Vec<_> = body
                .as_array()
                .expect("HTTP rows")
                .iter()
                .map(|r| r["id"].as_str().unwrap().to_owned())
                .collect();
            ids.sort();
            assert_eq!(ids, expected, "HTTP helper {i}/{entity}: {body}");
        }
        let (status, body) = rpc(
            &f.rt,
            APP,
            token,
            &select_ids("assignment"),
            json!([]),
            false,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(
            row_ids(&body["result"]).is_empty(),
            "sharing must not expose assignment records: {body}"
        );
    }
    // .own alone remains self-only despite active assignments.
    permissions(
        &f.rt,
        h1.user,
        std::iter::once(format!("app:{APP}:invoke"))
            .chain(OWNED.map(|e| format!("app:{APP}:{e}.read.own")))
            .collect(),
    )
    .await;
    for entity in OWNED {
        let (_, body) = rpc(
            &f.rt,
            APP,
            &f.helpers[0].0,
            &select_ids(entity),
            json!([]),
            false,
        )
        .await;
        assert_eq!(
            row_ids(&body["result"]),
            expected_ids(&[h1], entity),
            ".own/{entity}: {body}"
        );
    }
    f.rt.shutdown().await;
}

#[tokio::test]
async fn ending_or_deleting_the_last_assignment_revokes_on_the_next_statement() {
    let f = fixture().await;
    let h = &f.helpers[0].1;
    let s = &f.subjects[0];
    for end in [Some("1900-01-01"), Some("9999-12-31"), None] {
        let first = assign(&f.rt, h.enrollments[0], s.enrollments[0], None).await;
        let last = assign(&f.rt, h.enrollments[1], s.enrollments[1], None).await;
        for transaction in [false, true] {
            let (_, body) = rpc(
                &f.rt,
                APP,
                &f.helpers[0].0,
                &select_ids("note"),
                json!([]),
                transaction,
            )
            .await;
            assert_eq!(
                row_ids(&body["result"]),
                expected_ids(&[h, s, &f.alias], "note"),
                "before {end:?}: {body}"
            );
        }
        sqlx::query(&format!("DELETE FROM {APP}.assignment WHERE id = $1"))
            .bind(first)
            .execute(f.rt.pool())
            .await
            .unwrap();
        let (_, body) = rpc(
            &f.rt,
            APP,
            &f.helpers[0].0,
            &select_ids("note"),
            json!([]),
            false,
        )
        .await;
        assert_eq!(
            row_ids(&body["result"]),
            expected_ids(&[h, s, &f.alias], "note"),
            "one remaining edge: {body}"
        );
        match end {
            Some(date) => {
                sqlx::query(&format!(
                    "UPDATE {APP}.assignment SET end_date = $1::text::date WHERE id = $2"
                ))
                .bind(date)
                .bind(last)
                .execute(f.rt.pool())
                .await
                .unwrap();
            }
            None => {
                sqlx::query(&format!("DELETE FROM {APP}.assignment WHERE id = $1"))
                    .bind(last)
                    .execute(f.rt.pool())
                    .await
                    .unwrap();
            }
        }
        for entity in OWNED {
            for transaction in [false, true] {
                let (_, body) = rpc(
                    &f.rt,
                    APP,
                    &f.helpers[0].0,
                    &select_ids(entity),
                    json!([]),
                    transaction,
                )
                .await;
                assert_eq!(
                    row_ids(&body["result"]),
                    expected_ids(&[h], entity),
                    "revoked {end:?}/{entity}/tx={transaction}; self .own survives: {body}"
                );
            }
        }
    }
    f.rt.shutdown().await;
}

#[tokio::test]
async fn concurrent_committed_revocation_is_visible_inside_the_same_bun_callback() {
    let f = fixture().await;
    let h = &f.helpers[0].1;
    let s = &f.subjects[0];
    for end in [Some("1900-01-01"), Some("9999-12-31"), None] {
        let edge = assign(&f.rt, h.enrollments[0], s.enrollments[0], None).await;
        let mut barrier = f.rt.pool().begin().await.unwrap();
        sqlx::query("SELECT pg_advisory_xact_lock($1::bigint)")
            .bind(BARRIER)
            .execute(&mut *barrier)
            .await
            .unwrap();
        let client = f.rt.client.clone();
        let url = f.rt.url(&format!("/api/v1/apps/{APP}/rpc"));
        let token = f.helpers[0].0.clone();
        let call = tokio::spawn(async move {
            let response = client
                .post(url)
                .bearer_auth(token)
                .json(&json!({
                    "method": "revocation",
                    "params": {"sql": select_ids("note"), "barrier": BARRIER}
                }))
                .send()
                .await
                .unwrap();
            (response.status(), response.json::<Value>().await.unwrap())
        });
        // A queued lock proves the first read completed. No arbitrary sleep is
        // used to guess when the callback acquired its first MVCC snapshot.
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let waiting: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM pg_locks WHERE locktype = 'advisory'
                     AND classid = 0 AND objid = $1::oid AND objsubid = 1 AND NOT granted)",
                )
                .bind(BARRIER as i32)
                .fetch_one(f.rt.pool())
                .await
                .unwrap();
                if waiting {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("Bun callback did not reach the between-statements barrier");
        // This connection commits while the callback remains open and blocked.
        match end {
            Some(date) => {
                sqlx::query(&format!(
                    "UPDATE {APP}.assignment SET end_date = $1::text::date WHERE id = $2"
                ))
                .bind(date)
                .bind(edge)
                .execute(f.rt.pool())
                .await
                .unwrap();
            }
            None => {
                sqlx::query(&format!("DELETE FROM {APP}.assignment WHERE id = $1"))
                    .bind(edge)
                    .execute(f.rt.pool())
                    .await
                    .unwrap();
            }
        }
        barrier.commit().await.unwrap();
        let (status, body) = tokio::time::timeout(Duration::from_secs(10), call)
            .await
            .expect("callback did not resume")
            .unwrap();
        assert_eq!(status, StatusCode::OK, "{end:?}: {body}");
        assert_eq!(
            row_ids(&body["before"]),
            expected_ids(&[h, s, &f.alias], "note"),
            "{end:?}: {body}"
        );
        assert_eq!(
            row_ids(&body["after"]),
            expected_ids(&[h], "note"),
            "{end:?}: {body}"
        );
        assert_eq!(
            body["identity"]["rows"][0][0], body["finalIdentity"]["rows"][0][0],
            "both reads must run on the same callback connection: {body}"
        );
        assert_eq!(body["identity"]["rows"][0][1], "read committed", "{body}");
        assert_eq!(
            body["identity"]["rows"][0][2], "rootcx_app_executor",
            "{body}"
        );
    }
    f.rt.shutdown().await;
}

#[tokio::test]
async fn shared_reads_cannot_mutate_assignments_or_reparent_identity_chains() {
    let f = fixture().await;
    let h = &f.helpers[0].1;
    let s = &f.subjects[0];
    let active = assign(&f.rt, h.enrollments[0], s.enrollments[0], None).await;
    let ended = assign(
        &f.rt,
        h.enrollments[0],
        f.outsider.enrollments[0],
        Some("1900-01-01"),
    )
    .await;
    let mut keys = read_keys(true);
    // Make the relations visible so denied UPDATE/DELETE cannot pass merely
    // because SELECT filtered them before the mutation permission was checked.
    keys.push(format!("app:{APP}:assignment.read"));
    // Even forged .shared mutation keys must have no policy to honor them.
    for entity in TABLES {
        for action in ["create", "update", "delete"] {
            keys.push(format!("app:{APP}:{entity}.{action}.shared"));
        }
    }
    permissions(&f.rt, h.user, keys).await;
    let attacks = vec![
        (
            "create assignment",
            format!(
                "INSERT INTO {APP}.assignment (helper_enrollment_id, helped_enrollment_id) VALUES ($1, $2) RETURNING id"
            ),
            json!([h.enrollments[0], f.outsider.enrollments[0]]),
        ),
        (
            "reopen assignment",
            format!("UPDATE {APP}.assignment SET end_date = NULL WHERE id = $1 RETURNING id"),
            json!([ended]),
        ),
        (
            "reparent subject",
            format!(
                "UPDATE {APP}.assignment SET helped_enrollment_id = $1 WHERE id = $2 RETURNING id"
            ),
            json!([f.outsider.enrollments[0], active]),
        ),
        (
            "reparent grantee",
            format!(
                "UPDATE {APP}.assignment SET helper_enrollment_id = $1 WHERE id = $2 RETURNING id"
            ),
            json!([f.helpers[1].1.enrollments[0], active]),
        ),
        (
            "end assignment",
            format!(
                "UPDATE {APP}.assignment SET end_date = '1900-01-01' WHERE id = $1 RETURNING id"
            ),
            json!([active]),
        ),
        (
            "delete assignment",
            format!("DELETE FROM {APP}.assignment WHERE id = $1 RETURNING id"),
            json!([active]),
        ),
        (
            "create person identity",
            format!("INSERT INTO {APP}.person (core_user_id) VALUES ($1) RETURNING id"),
            json!([h.user]),
        ),
        (
            "change person identity",
            format!("UPDATE {APP}.person SET core_user_id = $1 WHERE id = $2 RETURNING id"),
            json!([h.user, s.person]),
        ),
        (
            "change own person identity",
            format!("UPDATE {APP}.person SET core_user_id = $1 WHERE id = $2 RETURNING id"),
            json!([s.user, h.person]),
        ),
        (
            "create enrollment",
            format!("INSERT INTO {APP}.enrollment (person_id) VALUES ($1) RETURNING id"),
            json!([s.person]),
        ),
        (
            "move enrollment",
            format!("UPDATE {APP}.enrollment SET person_id = $1 WHERE id = $2 RETURNING id"),
            json!([h.person, s.enrollments[0]]),
        ),
        (
            "create profile",
            format!("INSERT INTO {APP}.profile (person_id) VALUES ($1) RETURNING id"),
            json!([s.person]),
        ),
        (
            "move profile",
            format!("UPDATE {APP}.profile SET person_id = $1 WHERE id = $2 RETURNING id"),
            json!([h.person, s.profile]),
        ),
        (
            "create note",
            format!("INSERT INTO {APP}.note (enrollment_id) VALUES ($1) RETURNING id"),
            json!([s.enrollments[0]]),
        ),
        (
            "move note",
            format!("UPDATE {APP}.note SET enrollment_id = $1 WHERE id = $2 RETURNING id"),
            json!([h.enrollments[0], s.notes[0]]),
        ),
        (
            "edit shared note",
            format!("UPDATE {APP}.note SET label = 'stolen' WHERE id = $1 RETURNING id"),
            json!([s.notes[0]]),
        ),
        (
            "delete shared note",
            format!("DELETE FROM {APP}.note WHERE id = $1 RETURNING id"),
            json!([s.notes[0]]),
        ),
    ];
    let before = snapshot(&f.rt).await;
    for transaction in [false, true] {
        for (label, sql, args) in &attacks {
            let (status, body) =
                rpc(&f.rt, APP, &f.helpers[0].0, sql, args.clone(), transaction).await;
            assert_eq!(status, StatusCode::OK, "{label}/tx={transaction}: {body}");
            assert!(
                body["error"].as_str().is_some_and(|s| !s.is_empty())
                    || (body["result"]["rows"] == json!([]) && body["result"]["rowCount"] == 0),
                "{label}/tx={transaction} must error or affect zero rows: {body}"
            );
            assert_eq!(
                snapshot(&f.rt).await,
                before,
                "{label}/tx={transaction} changed persisted data"
            );
        }
    }
    // Add genuine own writes: editing self is allowed, but shared visibility
    // must not weaken the mutation predicates or ownership WITH CHECK.
    let mut keys = read_keys(true);
    for action in ["create", "update", "delete"] {
        keys.push(format!("app:{APP}:note.{action}.own"));
    }
    permissions(&f.rt, h.user, keys).await;
    for transaction in [false, true] {
        let (_, body) = rpc(
            &f.rt,
            APP,
            &f.helpers[0].0,
            &format!("UPDATE {APP}.note SET label = 'self edit' WHERE id = $1 RETURNING id::text"),
            json!([h.notes[0]]),
            transaction,
        )
        .await;
        assert_eq!(
            row_ids(&body["result"]),
            vec![h.notes[0].to_string()],
            "{body}"
        );
        let before = snapshot(&f.rt).await;
        for (label, sql, args) in [
            (
                "create for shared owner",
                format!("INSERT INTO {APP}.note (enrollment_id) VALUES ($1) RETURNING id"),
                json!([s.enrollments[0]]),
            ),
            (
                "edit visible shared row",
                format!("UPDATE {APP}.note SET label = 'stolen' WHERE id = $1 RETURNING id"),
                json!([s.notes[0]]),
            ),
            (
                "give own row away",
                format!("UPDATE {APP}.note SET enrollment_id = $1 WHERE id = $2 RETURNING id"),
                json!([s.enrollments[0], h.notes[0]]),
            ),
            (
                "delete visible shared row",
                format!("DELETE FROM {APP}.note WHERE id = $1 RETURNING id"),
                json!([s.notes[0]]),
            ),
        ] {
            let (_, body) = rpc(&f.rt, APP, &f.helpers[0].0, &sql, args, transaction).await;
            assert!(
                body["error"].is_string() || body["result"]["rowCount"] == 0,
                "{label}: {body}"
            );
            assert_eq!(snapshot(&f.rt).await, before, "{label}/tx={transaction}");
        }
    }
    f.rt.shutdown().await;
}

async fn executor<'a>(
    rt: &'a TestRuntime,
    app: Option<&str>,
    user: Option<Uuid>,
    delegated: bool,
    effective: &[String],
) -> sqlx::Transaction<'a, sqlx::Postgres> {
    let mut tx = rt.pool().begin().await.unwrap();
    sqlx::query(
        "SELECT set_config('rootcx.app_id', $1, true),
                set_config('rootcx.user_id', $2, true),
                set_config('rootcx.is_delegated', $3, true),
                set_config('rootcx.effective_perms', $4, true),
                set_config('rootcx.human_data_request', '', true)",
    )
    .bind(app.unwrap_or(""))
    .bind(user.map(|u| u.to_string()).unwrap_or_default())
    .bind(if delegated { "1" } else { "0" })
    .bind(effective.join(","))
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("SET LOCAL ROLE rootcx_app_executor")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx
}

// These names are used only for security probes: app code can explicitly call
// these public-to-the-executor entry points, independently of the table policy.
fn resolver_sql(entity: &str) -> String {
    format!("SELECT rootcx_system.\"rootcx_shared.{APP}.{entity}\"()::text AS id ORDER BY id")
}

#[tokio::test]
async fn resolvers_require_the_exact_target_shared_permission_and_bound_app_identity() {
    let f = fixture().await;
    let h = &f.helpers[0].1;
    let s = &f.subjects[0];
    assign(&f.rt, h.enrollments[0], s.enrollments[0], None).await;
    f.rt.install("assignment_bystander", "records").await;
    let (status, body) =
        f.rt.deploy(
            "assignment_bystander",
            &harness::make_tar_gz(&[("index.ts", BACKEND)]),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let mut keys = read_keys(false);
    keys.push("app:assignment_bystander:invoke".into());
    permissions(&f.rt, h.user, keys).await;

    for entity in OWNED {
        let keys = match entity {
            "person" => vec![s.user],
            "enrollment" | "profile" => vec![s.person, f.alias.person],
            "note" => s
                .enrollments
                .iter()
                .chain(&f.alias.enrollments)
                .copied()
                .collect(),
            _ => unreachable!(),
        };
        let expected: Vec<String> = keys
            .into_iter()
            .map(|id| id.to_string())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        for transaction in [false, true] {
            let (status, body) = rpc(
                &f.rt,
                APP,
                &f.helpers[0].0,
                &resolver_sql(entity),
                json!([]),
                transaction,
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{entity}/tx={transaction}: {body}");
            assert_eq!(
                row_ids(&body["result"]),
                expected,
                "{entity}: public resolver returns immediate ownership keys, never unrelated IDs: {body}"
            );
            let (_, body) = rpc(
                &f.rt,
                "assignment_bystander",
                &f.helpers[0].0,
                &resolver_sql(entity),
                json!([]),
                transaction,
            )
            .await;
            assert!(
                row_ids(&body["result"]).is_empty(),
                "foreign worker/{entity}/tx={transaction}: {body}"
            );
        }
    }

    // Permissions on adjacent owned entities cannot be used to enumerate the
    // note ownership keys; neither may an own key substitute for read.shared.
    for permission in [
        format!("app:{APP}:profile.read.shared"),
        format!("app:{APP}:note.read.own"),
    ] {
        permissions(
            &f.rt,
            h.user,
            vec![format!("app:{APP}:invoke"), permission.clone()],
        )
        .await;
        for transaction in [false, true] {
            let (_, body) = rpc(
                &f.rt,
                APP,
                &f.helpers[0].0,
                &resolver_sql("note"),
                json!([]),
                transaction,
            )
            .await;
            assert!(
                row_ids(&body["result"]).is_empty(),
                "{permission}/tx={transaction}: {body}"
            );
        }
    }
    permissions(&f.rt, h.user, read_keys(false)).await;
    for (label, app, uid, delegated, effective) in [
        ("no app", None, Some(h.user), false, vec![]),
        ("no user", Some(APP), None, false, vec![]),
        ("no delegated scope", Some(APP), Some(h.user), true, vec![]),
        (
            "wrong delegated entity",
            Some(APP),
            Some(h.user),
            true,
            vec![format!("app:{APP}:profile.read.shared")],
        ),
        (
            "exact delegated scope",
            Some(APP),
            Some(h.user),
            true,
            vec![format!("app:{APP}:note.read.shared")],
        ),
    ] {
        let mut tx = executor(&f.rt, app, uid, delegated, &effective).await;
        let ids: Vec<String> = sqlx::query_scalar(&resolver_sql("note"))
            .fetch_all(&mut *tx)
            .await
            .unwrap();
        tx.rollback().await.unwrap();
        let expected = if label == "exact delegated scope" {
            s.enrollments
                .iter()
                .chain(&f.alias.enrollments)
                .map(ToString::to_string)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
        } else {
            vec![]
        };
        assert_eq!(
            ids, expected,
            "{label}: resolver must honor the invocation ceiling"
        );
    }
    let resolvers: Vec<(String, String, bool)> = sqlx::query_as(
        "SELECT p.proname, p.provolatile::text,
                p.proacl IS NULL OR EXISTS (
                    SELECT 1 FROM aclexplode(p.proacl) a
                    WHERE a.grantee = 0 AND a.privilege_type = 'EXECUTE')
         FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace
         WHERE n.nspname = 'rootcx_system' AND starts_with(p.proname, $1)",
    )
    .bind(format!("rootcx_shared.{APP}."))
    .fetch_all(f.rt.pool())
    .await
    .unwrap();
    assert!(
        !resolvers.is_empty(),
        "fixture must exercise installed resolvers"
    );
    for (name, volatility, public_execute) in resolvers {
        assert_ne!(
            volatility, "i",
            "{name}: caller-dependent keys must not be constant-folded"
        );
        assert!(
            !public_execute,
            "{name}: PUBLIC must not enumerate shared ownership keys"
        );
    }
    f.rt.shutdown().await;
}

#[tokio::test]
async fn invalid_share_declarations_are_rejected_before_any_install_artifacts() {
    let rt = TestRuntime::boot().await;
    let mut cases = vec![];
    for (label, pointer, value) in [
        (
            "sensitive direct owner",
            "/dataContract/0/fields/0",
            json!({"name": "core_user_id", "type": "uuid", "owner": true, "sensitive": true}),
        ),
        (
            "sensitive delegated owner",
            "/dataContract/2/fields/0",
            json!({
                "name": "person_id", "type": "entity_link", "owner": true, "sensitive": true,
                "references": {"entity": "person", "field": "id"}
            }),
        ),
        (
            "missing grantee",
            "/dataContract/4/share/grantee",
            json!("absent"),
        ),
        (
            "missing subject",
            "/dataContract/4/share/subject",
            json!("absent"),
        ),
        (
            "missing active field",
            "/dataContract/4/share/activeWhen/isNull",
            json!("absent"),
        ),
        (
            "required active field",
            "/dataContract/4/fields/2",
            json!({"name": "end_date", "type": "date", "required": true}),
        ),
        (
            "non-date active field",
            "/dataContract/4/fields/2/type",
            json!("number"),
        ),
        (
            "same grantee and subject field",
            "/dataContract/4/share/subject",
            json!("helper_enrollment_id"),
        ),
        (
            "non-link grantee",
            "/dataContract/4/fields/0/type",
            json!("uuid"),
        ),
        (
            "non-link subject",
            "/dataContract/4/fields/1/type",
            json!("uuid"),
        ),
        (
            "non-PK link",
            "/dataContract/4/fields/1/references/field",
            json!("person_id"),
        ),
        (
            "external grantee",
            "/dataContract/4/fields/0/references/entity",
            json!("core:users"),
        ),
        (
            "external subject",
            "/dataContract/4/fields/1/references/entity",
            json!("foreign:enrollment"),
        ),
        (
            "missing link target",
            "/dataContract/4/fields/1/references/entity",
            json!("absent"),
        ),
        (
            "unowned target",
            "/dataContract/1/fields/0/owner",
            json!(false),
        ),
        (
            "unowned root",
            "/dataContract/0/fields/0/owner",
            json!(false),
        ),
        (
            "non-PK owner chain",
            "/dataContract/1/fields/0/references/field",
            json!("label"),
        ),
        (
            "ownership cycle",
            "/dataContract/1/fields/0/references/entity",
            json!("enrollment"),
        ),
        (
            "unsupported predicate",
            "/dataContract/4/share/activeWhen",
            json!({"equals": {"end_date": null}}),
        ),
        (
            "extra target config",
            "/dataContract/4/share",
            json!({
                "grantee": "helper_enrollment_id", "subject": "helped_enrollment_id",
                "activeWhen": {"isNull": "end_date"}, "targets": ["note"]
            }),
        ),
    ] {
        let mut bad = manifest();
        *bad.pointer_mut(pointer).unwrap() = value;
        cases.push((label, bad));
    }
    for (i, (label, mut bad)) in cases.into_iter().enumerate() {
        let app = format!("bad_assignment_{i}");
        bad["appId"] = json!(app);
        let (status, body) = rt.post_json("/api/v1/apps", &bad).await;
        assert!(
            matches!(
                status,
                StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY
            ),
            "{label} must be an install error: {status} {body}"
        );
        if label.starts_with("sensitive") {
            assert!(body.to_string().contains("nonsensitive owner field"), "{body}");
        }
        let artifacts: (i64, i64, i64, i64, i64, i64, i64) = sqlx::query_as(
            "SELECT
                (SELECT count(*) FROM pg_namespace WHERE nspname = $1),
                (SELECT count(*) FROM rootcx_system.apps WHERE id = $1),
                (SELECT count(*) FROM rootcx_system.app_installations WHERE app_id = $1),
                (SELECT count(*) FROM rootcx_system.rbac_permissions WHERE source_app = $1),
                (SELECT count(*) FROM rootcx_system.sensitive_fields WHERE app_id = $1),
                (SELECT count(*) FROM rootcx_system.row_access_contracts WHERE app_id = $1),
                (SELECT count(*) FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace
                 WHERE n.nspname = 'rootcx_system'
                   AND (starts_with(p.proname, 'rootcx_shared.' || $1 || '.')
                     OR starts_with(p.proname, 'rootcx_own.' || $1 || '.')))",
        )
        .bind(&app)
        .fetch_one(rt.pool())
        .await
        .unwrap();
        assert_eq!(
            artifacts,
            (0, 0, 0, 0, 0, 0, 0),
            "{label}: rejected install left artifacts"
        );
    }
    rt.shutdown().await;
}

async fn shared_artifacts(rt: &TestRuntime) -> (Vec<String>, i64) {
    let keys = sqlx::query_scalar(
        "SELECT key FROM rootcx_system.rbac_permissions
         WHERE source_app = $1 AND key LIKE '%.shared' ORDER BY key",
    )
    .bind(APP)
    .fetch_all(rt.pool())
    .await
    .unwrap();
    let functions = sqlx::query_scalar(
        "SELECT count(*) FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace
         WHERE n.nspname = 'rootcx_system' AND starts_with(p.proname, $1)",
    )
    .bind(format!("rootcx_shared.{APP}."))
    .fetch_one(rt.pool())
    .await
    .unwrap();
    (keys, functions)
}

#[tokio::test]
async fn redeploy_bootstrap_removal_and_reinstall_preserve_the_same_access_contract() {
    let f = fixture().await;
    let h = &f.helpers[0].1;
    let s = &f.subjects[0];
    assign(&f.rt, h.enrollments[0], s.enrollments[0], None).await;
    let (keys, _) = shared_artifacts(&f.rt).await;
    let mut expected_keys: Vec<_> = OWNED.map(|e| format!("app:{APP}:{e}.read.shared")).into();
    expected_keys.sort();
    assert_eq!(
        keys, expected_keys,
        "only read.shared is a grantable shared scope"
    );

    for phase in [
        "initial install",
        "redeploy",
        "bootstrap",
        "bootstrap again",
        "remove",
        "restore",
    ] {
        match phase {
            "redeploy" => {
                let mut updated = manifest();
                updated["version"] = json!("1.0.1");
                f.rt.install_manifest(&updated).await;
                let (status, body) =
                    f.rt.deploy(APP, &harness::make_tar_gz(&[("index.ts", BACKEND)]))
                        .await;
                assert_eq!(status, StatusCode::OK, "{body}");
            }
            "bootstrap" => {
                // Remove derived executable artifacts but retain durable manifest
                // and projection metadata, like an upgrade needing reconstruction.
                let policies: Vec<(String, String)> = sqlx::query_as(
                    "SELECT quote_ident(tablename), quote_ident(policyname)
                     FROM pg_policies WHERE schemaname = $1",
                )
                .bind(APP)
                .fetch_all(f.rt.pool())
                .await
                .unwrap();
                for (table, policy) in policies {
                    sqlx::query(&format!("DROP POLICY {policy} ON {APP}.{table}"))
                        .execute(f.rt.pool())
                        .await
                        .unwrap();
                }
                let functions: Vec<String> = sqlx::query_scalar(
                    "SELECT quote_ident(p.proname) FROM pg_proc p
                     JOIN pg_namespace n ON n.oid = p.pronamespace
                     WHERE n.nspname = 'rootcx_system' AND starts_with(p.proname, $1)",
                )
                .bind(format!("rootcx_shared.{APP}."))
                .fetch_all(f.rt.pool())
                .await
                .unwrap();
                assert!(
                    !functions.is_empty(),
                    "bootstrap must reconstruct real shared resolvers"
                );
                for function in functions {
                    sqlx::query(&format!(
                        "DROP FUNCTION IF EXISTS rootcx_system.{function}() CASCADE"
                    ))
                    .execute(f.rt.pool())
                    .await
                    .unwrap();
                }
                RbacExtension.bootstrap(f.rt.pool()).await.unwrap();
            }
            "bootstrap again" => RbacExtension.bootstrap(f.rt.pool()).await.unwrap(),
            "remove" => {
                let mut removed = manifest();
                removed["dataContract"][4]
                    .as_object_mut()
                    .unwrap()
                    .remove("share");
                f.rt.install_manifest(&removed).await;
                let (keys, functions) = shared_artifacts(&f.rt).await;
                assert!(
                    keys.is_empty(),
                    "removing share must retire its permissions: {keys:?}"
                );
                assert_eq!(
                    functions, 0,
                    "removing share must remove callable resolvers"
                );
                RbacExtension.bootstrap(f.rt.pool()).await.unwrap();
            }
            "restore" => f.rt.install_manifest(&manifest()).await,
            _ => {}
        }
        for entity in OWNED {
            let people = if phase == "remove" {
                vec![h]
            } else {
                vec![h, s, &f.alias]
            };
            for transaction in [false, true] {
                let (status, body) = rpc(
                    &f.rt,
                    APP,
                    &f.helpers[0].0,
                    &select_ids(entity),
                    json!([]),
                    transaction,
                )
                .await;
                assert_eq!(status, StatusCode::OK, "{phase}/{entity}: {body}");
                assert_eq!(
                    row_ids(&body["result"]),
                    expected_ids(&people, entity),
                    "{phase}/{entity}/tx={transaction}: {body}"
                );
            }
        }
        if phase != "remove" {
            assert_eq!(
                shared_artifacts(&f.rt).await.0,
                expected_keys,
                "{phase}: permission reconstruction"
            );
        }
    }
    let (status, body) = f.rt.delete_json(&format!("/api/v1/apps/{APP}")).await;
    assert!(status.is_success(), "uninstall: {body}");
    assert_eq!(
        shared_artifacts(&f.rt).await,
        (vec![], 0),
        "uninstall leaves no shared authority"
    );
    let contracts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rootcx_system.row_access_contracts WHERE app_id = $1",
    )
    .bind(APP)
    .fetch_one(f.rt.pool())
    .await
    .unwrap();
    assert_eq!(
        contracts, 0,
        "uninstall must remove durable share declarations"
    );
    f.rt.install_manifest(&manifest()).await;
    let (status, body) =
        f.rt.deploy(APP, &harness::make_tar_gz(&[("index.ts", BACKEND)]))
            .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let fresh_h = person(&f.rt, h.user, "reinstalled_helper").await;
    let fresh_s = person(&f.rt, s.user, "reinstalled_subject").await;
    for phase in ["no replacement assignment", "replacement assignment"] {
        if phase == "replacement assignment" {
            assign(&f.rt, fresh_h.enrollments[0], fresh_s.enrollments[0], None).await;
        }
        for entity in OWNED {
            let (_, body) = rpc(
                &f.rt,
                APP,
                &f.helpers[0].0,
                &select_ids(entity),
                json!([]),
                false,
            )
            .await;
            let people = if phase == "replacement assignment" {
                vec![&fresh_h, &fresh_s]
            } else {
                vec![&fresh_h]
            };
            assert_eq!(
                row_ids(&body["result"]),
                expected_ids(&people, entity),
                "{phase}/{entity}: {body}"
            );
        }
    }
    f.rt.shutdown().await;
}

fn plan_nodes<'a>(node: &'a Value, result: &mut Vec<&'a Value>) {
    result.push(node);
    if let Some(children) = node["Plans"].as_array() {
        for child in children {
            plan_nodes(child, result);
        }
    }
}

#[tokio::test]
async fn selective_shared_reads_have_real_rls_plans_over_two_hundred_thousand_rows() {
    let f = fixture().await;
    let h = &f.helpers[0].1;
    let s = &f.subjects[0];
    assign(&f.rt, h.enrollments[0], s.enrollments[0], None).await;
    // Bulk data is deliberately invisible. An unconfined or unindexed plan must
    // not pass because every generated row happens to be an authorized match.
    sqlx::query(&format!(
        "INSERT INTO {APP}.note (enrollment_id, label)
         SELECT $1, 'bulk invisible' FROM generate_series(1, 200000)"
    ))
    .bind(f.outsider.enrollments[0])
    .execute(f.rt.pool())
    .await
    .unwrap();
    sqlx::query(&format!(
        "INSERT INTO {APP}.assignment (helper_enrollment_id, helped_enrollment_id)
         SELECT $1, $2 FROM generate_series(1, 200000)"
    ))
    .bind(f.outsider.enrollments[0])
    .bind(f.outsider.enrollments[1])
    .execute(f.rt.pool())
    .await
    .unwrap();
    for entity in TABLES {
        sqlx::query(&format!("ANALYZE {APP}.{entity}"))
            .execute(f.rt.pool())
            .await
            .unwrap();
    }
    let total: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {APP}.note"))
        .fetch_one(f.rt.pool())
        .await
        .unwrap();
    assert!(total >= 200_000, "plan fixture has {total} notes");
    for own in [false, true] {
        permissions(&f.rt, h.user, read_keys(own)).await;
        let expected = if own {
            expected_ids(&[h, s, &f.alias], "note")
        } else {
            expected_ids(&[s, &f.alias], "note")
        };
        for transaction in [false, true] {
            let (_, body) = rpc(
                &f.rt,
                APP,
                &f.helpers[0].0,
                &select_ids("note"),
                json!([]),
                transaction,
            )
            .await;
            assert_eq!(
                row_ids(&body["result"]),
                expected,
                "large fixture/own={own}/tx={transaction}: {body}"
            );
        }
        // EXPLAIN is intentionally unavailable to app SQL. Pose the same real
        // executor identity on a harness connection; query the table without
        // substituting a hand-written ownership WHERE clause or forcing indexes.
        let mut tx = executor(&f.rt, Some(APP), Some(h.user), false, &[]).await;
        let identity: (String, bool) =
            sqlx::query_as("SELECT current_user::text, row_security_active($1::regclass)")
                .bind(format!("{APP}.note"))
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        assert_eq!(
            identity,
            ("rootcx_app_executor".into(), true),
            "EXPLAIN must exercise RLS"
        );
        let plan: Value = sqlx::query_scalar(&format!(
            "EXPLAIN (ANALYZE, BUFFERS, VERBOSE, FORMAT JSON) SELECT id FROM {APP}.note"
        ))
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        tx.rollback().await.unwrap();
        eprintln!("assignment shared RLS plan, own={own}, rows={total}:\n{plan:#}");
        assert_eq!(
            plan[0]["Plan"]["Actual Rows"].as_u64(),
            Some(expected.len() as u64),
            "the analyzed plan must return the same authorized set: {plan:#}"
        );
        let mut nodes = vec![];
        plan_nodes(&plan[0]["Plan"], &mut nodes);
        let scans: Vec<_> = nodes
            .iter()
            .filter(|n| n["Relation Name"] == "note")
            .collect();
        assert!(
            !scans.is_empty(),
            "plan must actually scan the target relation: {plan:#}"
        );
        // Dynamic broad and own grants are OR-ed by RLS. PostgreSQL may scan
        // the target table for an unfiltered SELECT; forcing an ownership-key
        // array for broad readers breaks INSERT RETURNING (new keys are absent
        // from a STABLE resolver's snapshot). The cost invariant is a hashed
        // set evaluated once, never a join or linear array search per target row.
        assert!(
            scans.iter().any(|n| n["Filter"].as_str().is_some_and(|f| f.contains("hashed SubPlan"))),
            "shared membership must hash the resolved set once: {plan:#}"
        );
        for node in nodes.iter().filter(|n| matches!(
            n["Node Type"].as_str(), Some("Function Scan" | "ProjectSet")
        )) {
            assert!(
                node["Actual Loops"].as_f64().unwrap_or(0.0) <= 1.0,
                "shared/owner resolvers must not run once per target row: {node:#}"
            );
        }
    }
    f.rt.shutdown().await;
}
