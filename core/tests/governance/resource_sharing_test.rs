//! Resource shares across real manifest installation, HTTP and Bun SQL/RLS.
//! These cases catch identity widening, inferred descendant grants, resolver
//! context bypasses, shared writes, stale callback snapshots and lifecycle drift.

use crate::harness::{self, TestRuntime};
use reqwest::{Method, StatusCode};
use rootcx_core::extensions::{RuntimeExtension, rbac::RbacExtension};
use serde_json::{Value, json};
use std::{collections::BTreeSet, time::Duration};
use uuid::Uuid;

const APP: &str = "resource_sharing";
const FOREIGN: &str = "resource_bystander";
const LEGACY: &str = "resource_legacy";
const BARRIER: i64 = 846_262;
const TABLES: [&str; 7] = [
    "member",
    "project",
    "folder",
    "document",
    "omitted",
    "assignment",
    "legacy_assignment",
];

const BACKEND: &[u8] = br#"
serve({ rpc: {
  sql: async (params, _caller, ctx) => {
    try {
      const run = async (db) => db.sql(params.sql, params.args ?? []);
      return { result: params.transaction ? await ctx.transaction(run) : await run(ctx) };
    } catch (error) { return { error: error.message }; }
  },
  revocation: async (params, _caller, ctx) => ctx.transaction(async (tx) => {
    const before = await tx.sql(params.sql);
    const identity = await tx.sql(
      "SELECT pg_backend_pid(), current_setting('transaction_isolation'), current_user::text"
    );
    await tx.sql("SELECT pg_advisory_xact_lock($1::bigint)", [params.barrier]);
    const after = await tx.sql(params.sql);
    const finalIdentity = await tx.sql("SELECT pg_backend_pid()");
    return { before, after, identity, finalIdentity };
  }),
} });
"#;

fn link(name: &str, target: &str, owner: bool) -> Value {
    json!({
        "name": name, "type": "entity_link", "owner": owner,
        "references": {"entity": target, "field": "id"}
    })
}

fn identity_share() -> Value {
    // Deliberately omit scope: historical declarations must keep identity semantics.
    json!({
        "grantee": "member_id", "subject": "project_id",
        "activeWhen": {"isNull": "end_date"}
    })
}

fn manifest(owned: bool) -> Value {
    json!({
        "appId": APP, "name": "Resource sharing", "version": "1.0.0",
        "dataContract": [
            {"entityName": "member", "fields": [
                link("core_user_id", "core:users", true)
            ]},
            {"entityName": "project", "fields": [
                link("member_id", "member", owned), {"name": "label", "type": "text"}
            ]},
            {"entityName": "folder", "fields": [
                link("project_id", "project", owned)
            ]},
            {"entityName": "document", "fields": [
                link("folder_id", "folder", owned),
                {"name": "label", "type": "text"},
                {"name": "secret", "type": "text", "sensitive": true}
            ]},
            {"entityName": "omitted", "fields": [
                link("project_id", "project", owned)
            ]},
            {"entityName": "assignment", "share": {
                "grantee": "member_id", "subject": "project_id",
                "activeWhen": {"isNull": "end_date"}, "scope": "resource",
                "targets": [{"entity": "document", "via": ["folder_id", "project_id"]}]
            }, "fields": [
                link("member_id", "member", false), link("project_id", "project", false),
                {"name": "end_date", "type": "date"}
            ]},
            {"entityName": "legacy_assignment", "fields": [
                link("member_id", "member", false), link("project_id", "project", false),
                {"name": "end_date", "type": "date"}
            ]}
        ]
    })
}

fn contract_mut<'a>(manifest: &'a mut Value, entity: &str) -> &'a mut Value {
    manifest["dataContract"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|entry| entry["entityName"] == entity)
        .unwrap()
}

struct Resource {
    project: Uuid,
    folder: Uuid,
    documents: Vec<Uuid>,
}

struct Fixture {
    rt: TestRuntime,
    token: String,
    user: Uuid,
    owner_user: Uuid,
    member: Uuid,
    owner_member: Uuid,
    shared: Resource,
    sibling: Resource,
    own: Resource,
    edge: Uuid,
}

async fn scoped_user(rt: &TestRuntime, label: &str) -> (String, Uuid) {
    let email = format!("{label}@resource-sharing.test");
    let token = rt.create_user(&email).await;
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
    let role = format!("resource_{}", id.simple());
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
        .bind(format!("resource_{}", user.simple()))
        .execute(rt.pool())
        .await
        .unwrap();
}

fn read_keys(app: &str, entities: &[&str]) -> Vec<String> {
    std::iter::once(format!("app:{app}:invoke"))
        .chain(
            entities
                .iter()
                .map(|e| format!("app:{app}:{e}.read.shared")),
        )
        .collect()
}

async fn insert(rt: &TestRuntime, app: &str, entity: &str, payload: Value) -> Uuid {
    let row = rt.create(app, entity, &payload).await;
    Uuid::parse_str(row["id"].as_str().unwrap()).unwrap()
}

async fn resource(rt: &TestRuntime, app: &str, member: Option<Uuid>, label: &str) -> Resource {
    let project = insert(
        rt,
        app,
        "project",
        json!({"member_id": member, "label": label}),
    )
    .await;
    let folder = insert(rt, app, "folder", json!({"project_id": project})).await;
    let mut documents = vec![];
    for suffix in ["a", "b"] {
        documents.push(insert(rt, app, "document", json!({
            "folder_id": folder, "label": format!("{label}-{suffix}"), "secret": "classified"
        })).await);
    }
    insert(rt, app, "omitted", json!({"project_id": project})).await;
    Resource {
        project,
        folder,
        documents,
    }
}

async fn assign(rt: &TestRuntime, member: Uuid, project: Uuid, end: Option<&str>) -> Uuid {
    insert(
        rt,
        APP,
        "assignment",
        json!({
            "member_id": member, "project_id": project, "end_date": end
        }),
    )
    .await
}

async fn deploy(rt: &TestRuntime, app: &str) {
    let (status, body) = rt
        .deploy(app, &harness::make_tar_gz(&[("index.ts", BACKEND)]))
        .await;
    assert_eq!(status, StatusCode::OK, "fixture worker {app}: {body}");
}

async fn fixture(owned: bool) -> Fixture {
    let rt = TestRuntime::boot().await;
    rt.install_manifest(&manifest(owned)).await;
    let (token, user) = scoped_user(&rt, "helper").await;
    let (_, owner_user) = scoped_user(&rt, "owner").await;
    let member = insert(&rt, APP, "member", json!({"core_user_id": user})).await;
    let owner_member = insert(&rt, APP, "member", json!({"core_user_id": owner_user})).await;
    // The owner-free case has neither an owner declaration nor an identity value
    // on the shared root. Its descendants also declare no ownership.
    let owner = owned.then_some(owner_member);
    let shared = resource(&rt, APP, owner, "shared").await;
    let sibling = resource(&rt, APP, owner, "sibling").await;
    let own = resource(&rt, APP, Some(member), "self").await;
    let edge = assign(&rt, member, shared.project, None).await;
    permissions(&rt, user, read_keys(APP, &["project", "document"])).await;
    deploy(&rt, APP).await;
    Fixture {
        rt,
        token,
        user,
        owner_user,
        member,
        owner_member,
        shared,
        sibling,
        own,
        edge,
    }
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

fn select_ids(app: &str, entity: &str) -> String {
    format!("SELECT id::text FROM {app}.{entity} ORDER BY id")
}

fn resolver_sql(entity: &str) -> String {
    format!("SELECT rootcx_system.\"rootcx_shared.{APP}.{entity}\"()::text AS id ORDER BY id")
}

fn ids(values: &[Uuid]) -> Vec<String> {
    values
        .iter()
        .map(ToString::to_string)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn sql_ids(result: &Value) -> Vec<String> {
    result["rows"]
        .as_array()
        .unwrap_or_else(|| panic!("expected SQL rows: {result}"))
        .iter()
        .map(|row| row[0].as_str().expect("text ID").to_owned())
        .collect()
}

fn http_ids(body: &Value) -> Vec<String> {
    let mut ids: Vec<_> = body
        .as_array()
        .unwrap_or_else(|| panic!("expected HTTP rows: {body}"))
        .iter()
        .map(|row| row["id"].as_str().unwrap().to_owned())
        .collect();
    ids.sort();
    ids
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
async fn resource_reads_are_exact_roots_and_explicit_multihop_targets_with_safe_projection() {
    let f = fixture(true).await;
    // Duplicate edges are set membership; a future end is already inactive.
    assign(&f.rt, f.member, f.shared.project, None).await;
    assign(&f.rt, f.member, f.sibling.project, Some("9999-12-31")).await;
    assign(&f.rt, f.owner_member, f.sibling.project, None).await;
    insert(
        &f.rt,
        APP,
        "assignment",
        json!({"project_id": f.sibling.project}),
    )
    .await;
    insert(&f.rt, APP, "assignment", json!({"member_id": f.member})).await;
    // Even a manually stored shared key cannot manufacture an omitted target.
    permissions(&f.rt, f.user, read_keys(APP, &TABLES)).await;
    for entity in TABLES {
        let expected = match entity {
            "project" => ids(&[f.shared.project]),
            "document" => ids(&f.shared.documents),
            _ => vec![],
        };
        for transaction in [false, true] {
            let (status, body) = rpc(
                &f.rt,
                APP,
                &f.token,
                &select_ids(APP, entity),
                json!([]),
                transaction,
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{entity}/tx={transaction}: {body}");
            assert_eq!(
                sql_ids(&body["result"]),
                expected,
                "{entity}/tx={transaction}: sibling, self and undeclared descendants stay private"
            );
        }
        // Omitted entities have no grantable read.shared permission, so their
        // SQL denial above is the relevant boundary; HTTP may reject at its gate.
        if matches!(entity, "project" | "document") {
            let (status, body) =
                f.rt.request_as(
                    Method::GET,
                    &format!("/api/v1/apps/{APP}/collections/{entity}"),
                    &f.token,
                    None,
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{entity}: {body}");
            assert_eq!(
                http_ids(&body),
                expected,
                "HTTP and SQL must agree for {entity}"
            );
            for row in body.as_array().unwrap() {
                assert!(
                    row["label"].is_string(),
                    "safe fields must remain usable: {row}"
                );
                assert!(
                    row.get("secret").is_none(),
                    "resource sharing leaked sensitive data: {row}"
                );
            }
        }
    }
    let (status, body) = rpc(
        &f.rt,
        APP,
        &f.token,
        &format!("SELECT secret FROM {APP}.document"),
        json!([]),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|e| e.contains("permission denied")),
        "resource sharing must not grant sensitive SQL columns: {body}"
    );
    f.rt.shutdown().await;
}

#[tokio::test]
async fn owner_free_resources_need_only_exact_target_permission_and_resolvers_guard_context() {
    let f = fixture(false).await;
    // No project, folder or member read grant is needed to traverse the path.
    permissions(&f.rt, f.user, read_keys(APP, &["document"])).await;
    for entity in ["project", "document"] {
        let expected = if entity == "document" {
            ids(&f.shared.documents)
        } else {
            vec![]
        };
        for query in [select_ids(APP, entity), resolver_sql(entity)] {
            let (status, body) = rpc(&f.rt, APP, &f.token, &query, json!([]), false).await;
            assert_eq!(status, StatusCode::OK, "{query}: {body}");
            assert_eq!(
                sql_ids(&body["result"]),
                expected,
                "resource-only resolvers return target primary keys, with exact entity gates"
            );
        }
    }
    let (status, body) =
        f.rt.request_as(
            Method::GET,
            &format!("/api/v1/apps/{APP}/collections/document"),
            &f.token,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        http_ids(&body),
        ids(&f.shared.documents),
        "owner-free HTTP target read"
    );

    for key in [
        format!("app:{APP}:project.read.shared"),
        format!("app:{APP}:document.read.own"),
    ] {
        permissions(
            &f.rt,
            f.user,
            vec![format!("app:{APP}:invoke"), key.clone()],
        )
        .await;
        for query in [select_ids(APP, "document"), resolver_sql("document")] {
            let (_, body) = rpc(&f.rt, APP, &f.token, &query, json!([]), false).await;
            assert!(
                sql_ids(&body["result"]).is_empty(),
                "{key} cannot authorize {query}: {body}"
            );
        }
    }
    f.rt.install(FOREIGN, "records").await;
    deploy(&f.rt, FOREIGN).await;
    let mut keys = read_keys(APP, &["project", "document"]);
    keys.push(format!("app:{FOREIGN}:invoke"));
    permissions(&f.rt, f.user, keys).await;
    for entity in ["project", "document"] {
        let expected = if entity == "project" {
            ids(&[f.shared.project])
        } else {
            ids(&f.shared.documents)
        };
        let (_, body) = rpc(
            &f.rt,
            APP,
            &f.token,
            &resolver_sql(entity),
            json!([]),
            false,
        )
        .await;
        assert_eq!(
            sql_ids(&body["result"]),
            expected,
            "owner-free {entity} resolver must return primary keys"
        );
        let (status, body) =
            f.rt.request_as(
                Method::GET,
                &format!("/api/v1/apps/{APP}/collections/{entity}"),
                &f.token,
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "owner-free {entity}: {body}");
        assert_eq!(http_ids(&body), expected, "owner-free {entity} HTTP read");
        for transaction in [false, true] {
            let (_, body) = rpc(
                &f.rt,
                FOREIGN,
                &f.token,
                &resolver_sql(entity),
                json!([]),
                transaction,
            )
            .await;
            assert!(
                sql_ids(&body["result"]).is_empty(),
                "foreign app cannot enumerate resource keys for {entity}/tx={transaction}: {body}"
            );
        }
    }
    // Exercise the callable resolver under the real executor role, including
    // context states an authenticated HTTP route normally cannot construct.
    for (label, app, user, delegated, effective, allowed) in [
        ("missing app", "", f.user.to_string(), false, "", false),
        ("missing user", APP, String::new(), false, "", false),
        ("empty delegation", APP, f.user.to_string(), true, "", false),
        (
            "adjacent delegation",
            APP,
            f.user.to_string(),
            true,
            "app:resource_sharing:project.read.shared",
            false,
        ),
        (
            "exact delegation",
            APP,
            f.user.to_string(),
            true,
            "app:resource_sharing:document.read.shared",
            true,
        ),
    ] {
        let mut tx = f.rt.pool().begin().await.unwrap();
        sqlx::query(
            "SELECT set_config('rootcx.app_id', $1, true),
                    set_config('rootcx.user_id', $2, true),
                    set_config('rootcx.is_delegated', $3, true),
                    set_config('rootcx.effective_perms', $4, true),
                    set_config('rootcx.human_data_request', '', true)",
        )
        .bind(app)
        .bind(user)
        .bind(if delegated { "1" } else { "0" })
        .bind(effective)
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query("SET LOCAL ROLE rootcx_app_executor")
            .execute(&mut *tx)
            .await
            .unwrap();
        let actual: Vec<String> = sqlx::query_scalar(&resolver_sql("document"))
            .fetch_all(&mut *tx)
            .await
            .unwrap();
        tx.rollback().await.unwrap();
        assert_eq!(
            actual,
            if allowed {
                ids(&f.shared.documents)
            } else {
                vec![]
            },
            "{label}"
        );
    }
    f.rt.shutdown().await;
}

#[tokio::test]
async fn caller_manipulation_and_shared_mutations_cannot_change_persisted_resources_or_edges() {
    let f = fixture(true).await;
    let ended = assign(&f.rt, f.member, f.sibling.project, Some("1900-01-01")).await;
    let mut keys = read_keys(APP, &["project", "document"]);
    keys.push(format!("app:{APP}:assignment.read"));
    for entity in ["project", "document", "assignment"] {
        for action in ["create", "update", "delete"] {
            keys.push(format!("app:{APP}:{entity}.{action}.shared"));
        }
    }
    permissions(&f.rt, f.user, keys).await;
    let before = snapshot(&f.rt).await;
    let attacks = [
        (
            "create shared child",
            format!(
                "INSERT INTO {APP}.document (folder_id, label) VALUES ($1, 'forged') RETURNING id"
            ),
            json!([f.shared.folder]),
        ),
        (
            "edit shared child",
            format!("UPDATE {APP}.document SET label = 'forged' WHERE id = $1 RETURNING id"),
            json!([f.shared.documents[0]]),
        ),
        (
            "delete shared child",
            format!("DELETE FROM {APP}.document WHERE id = $1 RETURNING id"),
            json!([f.shared.documents[0]]),
        ),
        (
            "reparent shared child",
            format!("UPDATE {APP}.document SET folder_id = $1 WHERE id = $2 RETURNING id"),
            json!([f.own.folder, f.shared.documents[0]]),
        ),
        (
            "create assignment",
            format!(
                "INSERT INTO {APP}.assignment (member_id, project_id) VALUES ($1, $2) RETURNING id"
            ),
            json!([f.member, f.sibling.project]),
        ),
        (
            "reopen assignment",
            format!("UPDATE {APP}.assignment SET end_date = NULL WHERE id = $1 RETURNING id"),
            json!([ended]),
        ),
        (
            "retarget assignment",
            format!("UPDATE {APP}.assignment SET project_id = $1 WHERE id = $2 RETURNING id"),
            json!([f.sibling.project, f.edge]),
        ),
        (
            "replace grantee",
            format!("UPDATE {APP}.assignment SET member_id = $1 WHERE id = $2 RETURNING id"),
            json!([f.owner_member, f.edge]),
        ),
        (
            "delete assignment",
            format!("DELETE FROM {APP}.assignment WHERE id = $1 RETURNING id"),
            json!([f.edge]),
        ),
    ];
    for transaction in [false, true] {
        for (label, query, args) in &attacks {
            let (status, body) = rpc(&f.rt, APP, &f.token, query, args.clone(), transaction).await;
            assert_eq!(status, StatusCode::OK, "{label}/tx={transaction}: {body}");
            assert!(
                body["error"].as_str().is_some_and(|e| !e.is_empty())
                    || (body["result"]["rowCount"] == 0 && body["result"]["rows"] == json!([])),
                "{label}/tx={transaction} must error or affect zero rows: {body}"
            );
            assert_eq!(
                snapshot(&f.rt).await,
                before,
                "{label}/tx={transaction} changed data"
            );
        }
        for (query, args) in [
            (
                "SELECT set_config('rootcx.user_id', $1, true)",
                json!([f.owner_user]),
            ),
            (
                "SELECT set_config('rootcx.effective_perms', '*', true)",
                json!([]),
            ),
            (
                "SELECT set_config('rootcx.human_data_request', '1', true)",
                json!([]),
            ),
            ("SET ROLE postgres", json!([])),
        ] {
            let (status, body) = rpc(&f.rt, APP, &f.token, query, args, transaction).await;
            assert_eq!(status, StatusCode::OK, "{query}/tx={transaction}: {body}");
            assert!(
                body["error"].is_string(),
                "caller cannot forge SQL authority: {query}: {body}"
            );
        }
        let (_, body) = rpc(
            &f.rt,
            APP,
            &f.token,
            &select_ids(APP, "document"),
            json!([]),
            transaction,
        )
        .await;
        assert_eq!(
            sql_ids(&body["result"]),
            ids(&f.shared.documents),
            "failed caller manipulation must not contaminate the next request"
        );
    }
    for (method, entity, id, payload) in [
        (
            Method::POST,
            "document",
            None,
            Some(json!({"folder_id": f.shared.folder, "label": "forged"})),
        ),
        (
            Method::PATCH,
            "document",
            Some(f.shared.documents[0]),
            Some(json!({"label": "forged"})),
        ),
        (
            Method::DELETE,
            "document",
            Some(f.shared.documents[0]),
            None,
        ),
        (
            Method::PATCH,
            "project",
            Some(f.shared.project),
            Some(json!({"member_id": f.member})),
        ),
        (
            Method::PATCH,
            "assignment",
            Some(ended),
            Some(json!({"end_date": null})),
        ),
    ] {
        let path = format!(
            "/api/v1/apps/{APP}/collections/{entity}{}",
            id.map(|id| format!("/{id}")).unwrap_or_default()
        );
        let (status, body) =
            f.rt.request_as(method.clone(), &path, &f.token, payload.as_ref())
                .await;
        assert!(
            matches!(status, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
            "{method} {path} must deny shared writes: {status} {body}"
        );
        assert_eq!(
            snapshot(&f.rt).await,
            before,
            "{method} {path} changed data"
        );
    }
    f.rt.shutdown().await;
}

#[tokio::test]
async fn committed_resource_revocation_reaches_the_next_statement_in_a_bun_callback() {
    let f = fixture(false).await;
    for delete in [false, true] {
        let edge = if delete {
            assign(&f.rt, f.member, f.shared.project, None).await
        } else {
            f.edge
        };
        let mut barrier = f.rt.pool().begin().await.unwrap();
        sqlx::query("SELECT pg_advisory_xact_lock($1::bigint)")
            .bind(BARRIER)
            .execute(&mut *barrier)
            .await
            .unwrap();
        let client = f.rt.client.clone();
        let url = f.rt.url(&format!("/api/v1/apps/{APP}/rpc"));
        let token = f.token.clone();
        let call = tokio::spawn(async move {
            let response = client
                .post(url)
                .bearer_auth(token)
                .json(&json!({
                    "method": "revocation",
                    "params": {"sql": select_ids(APP, "document"), "barrier": BARRIER}
                }))
                .send()
                .await
                .unwrap();
            (response.status(), response.json::<Value>().await.unwrap())
        });
        // A queued lock proves the first statement completed; no timing guess.
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
        .expect("Bun callback did not reach its between-statements barrier");
        let query = if delete {
            format!("DELETE FROM {APP}.assignment WHERE id = $1")
        } else {
            format!("UPDATE {APP}.assignment SET end_date = '9999-12-31' WHERE id = $1")
        };
        // A separate admin connection commits while the callback stays open.
        sqlx::query(&query)
            .bind(edge)
            .execute(f.rt.pool())
            .await
            .unwrap();
        barrier.commit().await.unwrap();
        let (status, body) = tokio::time::timeout(Duration::from_secs(10), call)
            .await
            .expect("callback did not resume")
            .unwrap();
        assert_eq!(status, StatusCode::OK, "delete={delete}: {body}");
        assert_eq!(
            sql_ids(&body["before"]),
            ids(&f.shared.documents),
            "delete={delete}: {body}"
        );
        assert!(
            sql_ids(&body["after"]).is_empty(),
            "delete={delete}: stale shared resource: {body}"
        );
        assert_eq!(
            body["identity"]["rows"][0][0], body["finalIdentity"]["rows"][0][0],
            "both reads must use the same callback connection: {body}"
        );
        assert_eq!(body["identity"]["rows"][0][1], "read committed", "{body}");
        assert_eq!(
            body["identity"]["rows"][0][2], "rootcx_app_executor",
            "{body}"
        );
        for entity in ["project", "document"] {
            let (status, body) =
                f.rt.request_as(
                    Method::GET,
                    &format!("/api/v1/apps/{APP}/collections/{entity}"),
                    &f.token,
                    None,
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{entity}/delete={delete}: {body}");
            assert!(
                http_ids(&body).is_empty(),
                "HTTP retained revoked {entity}: {body}"
            );
        }
    }
    f.rt.shutdown().await;
}

async fn projection(rt: &TestRuntime, app: &str) -> (i32, Value) {
    sqlx::query_as(
        "SELECT version, entities FROM rootcx_system.row_access_contracts WHERE app_id = $1",
    )
    .bind(app)
    .fetch_one(rt.pool())
    .await
    .unwrap()
}

async fn shared_keys(rt: &TestRuntime, app: &str) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT key FROM rootcx_system.rbac_permissions
         WHERE source_app = $1 AND key LIKE '%.shared' ORDER BY key",
    )
    .bind(app)
    .fetch_all(rt.pool())
    .await
    .unwrap()
}

#[tokio::test]
async fn resource_bootstrap_refuses_a_downgraded_contract_without_reinterpreting_it() {
    let f = fixture(false).await;
    let (_, entities) = projection(&f.rt, APP).await;
    sqlx::query("UPDATE rootcx_system.row_access_contracts SET version = 1 WHERE app_id = $1")
        .bind(APP)
        .execute(f.rt.pool())
        .await
        .unwrap();
    let error = RbacExtension.bootstrap(f.rt.pool()).await
        .expect_err("resource declarations must not be accepted as identity contract v1");
    assert!(error.to_string().contains("requires contract version 2"), "{error}");
    assert_eq!(projection(&f.rt, APP).await, (1, entities),
        "a failed bootstrap must not silently fall back to the stored manifest");
    // Restore the valid contract and confirm the original data is still governed.
    sqlx::query("UPDATE rootcx_system.row_access_contracts SET version = 2 WHERE app_id = $1")
        .bind(APP)
        .execute(f.rt.pool())
        .await
        .unwrap();
    RbacExtension.bootstrap(f.rt.pool()).await.unwrap();
    let (_, body) = rpc(&f.rt, APP, &f.token, &select_ids(APP, "document"), json!([]), false).await;
    assert_eq!(sql_ids(&body["result"]), ids(&f.shared.documents));
    f.rt.shutdown().await;
}

#[tokio::test]
async fn resource_reads_follow_live_intermediate_links_without_copying_data() {
    let f = fixture(false).await;
    // Only this relationship changes, not the assignment or any document.
    let before: Value = sqlx::query_scalar(&format!(
        "SELECT jsonb_agg(to_jsonb(d) ORDER BY id) FROM {APP}.document d"
    )).fetch_one(f.rt.pool()).await.unwrap();
    sqlx::query(&format!("UPDATE {APP}.folder SET project_id = $1 WHERE id = $2"))
        .bind(f.sibling.project).bind(f.shared.folder)
        .execute(f.rt.pool()).await.unwrap();
    let (_, body) = rpc(&f.rt, APP, &f.token, &select_ids(APP, "document"), json!([]), false).await;
    assert!(sql_ids(&body["result"]).is_empty(),
        "moving the folder out must revoke its descendants: {body}");
    sqlx::query(&format!("UPDATE {APP}.folder SET project_id = $1 WHERE id = $2"))
        .bind(f.shared.project).bind(f.sibling.folder)
        .execute(f.rt.pool()).await.unwrap();
    let (_, body) = rpc(&f.rt, APP, &f.token, &select_ids(APP, "document"), json!([]), true).await;
    assert_eq!(sql_ids(&body["result"]), ids(&f.sibling.documents),
        "moving another folder in must expose its live documents");
    let after: Value = sqlx::query_scalar(&format!(
        "SELECT jsonb_agg(to_jsonb(d) ORDER BY id) FROM {APP}.document d"
    )).fetch_one(f.rt.pool()).await.unwrap();
    assert_eq!(after, before, "authorization must not copy or rewrite business records");
    f.rt.shutdown().await;
}

/// Revoking a resource edge must affect its grantee, not every reader sharing
/// the same role or resource. Exercise the documented public setup, not SQL RBAC.
#[tokio::test]
async fn public_resource_revocation_removes_only_one_of_two_readers() {
    let rt = TestRuntime::boot().await;
    rt.install_manifest(&manifest(false)).await;
    deploy(&rt, APP).await;
    let reader_keys = read_keys(APP, &["project", "document"]);
    let manager_keys: Vec<String> = ["read", "create", "update", "delete"]
        .map(|action| format!("app:{APP}:assignment.{action}"))
        .into_iter()
        .collect();
    for (role, keys) in [("resource_reader", &reader_keys), ("resource_manager", &manager_keys)] {
        let (status, body) = rt.post_json("/api/v1/roles", &json!({
            "name": role, "permissions": keys
        })).await;
        assert_eq!(status, StatusCode::OK, "create {role}: {body}");
    }

    let mut principals = Vec::new();
    for (label, role, expected_keys) in [
        ("manager", "resource_manager", &manager_keys),
        ("reader_a", "resource_reader", &reader_keys),
        ("reader_b", "resource_reader", &reader_keys),
    ] {
        let token = rt.create_user(&format!("{label}@public-resource.test")).await;
        let (status, me) = rt.request_as(Method::GET, "/api/v1/auth/me", &token, None).await;
        assert_eq!(status, StatusCode::OK, "{label}: {me}");
        let user = Uuid::parse_str(me["id"].as_str().unwrap()).unwrap();
        // Accounts may receive default roles. Remove them through the public
        // administrator path so broad grants cannot hide a sharing regression.
        let (status, assignments) = rt.get_json("/api/v1/roles/assignments").await;
        assert_eq!(status, StatusCode::OK, "{assignments}");
        for assignment in assignments.as_array().unwrap().iter()
            .filter(|assignment| assignment["userId"] == user.to_string())
        {
            let (status, body) = rt.post_json("/api/v1/roles/revoke", &json!({
                "userId": user, "role": assignment["role"]
            })).await;
            assert_eq!(status, StatusCode::OK, "remove default role for {label}: {body}");
        }
        let (status, body) = rt.post_json("/api/v1/roles/assign", &json!({
            "userId": user, "role": role
        })).await;
        assert_eq!(status, StatusCode::OK, "assign {role}: {body}");
        let (status, effective) = rt.request_as(Method::GET, "/api/v1/permissions", &token, None).await;
        assert_eq!(status, StatusCode::OK, "{label}: {effective}");
        let actual: BTreeSet<_> = effective["permissions"].as_array().unwrap()
            .iter().map(|key| key.as_str().unwrap().to_string()).collect();
        assert_eq!(actual, expected_keys.iter().cloned().collect(),
            "{label} must have only the intended permissions");
        principals.push((token, user));
    }

    let manager = &principals[0].0;
    let readers = &principals[1..];
    let shared = resource(&rt, APP, None, "shared").await;
    let _private = resource(&rt, APP, None, "private").await;
    let assignment_path = format!("/api/v1/apps/{APP}/collections/assignment");
    let mut edges = Vec::new();
    for (_, user) in readers {
        let member = insert(&rt, APP, "member", json!({"core_user_id": user})).await;
        let (status, body) = rt.request_as(Method::POST, &assignment_path, manager, Some(&json!({
            "member_id": member, "project_id": shared.project, "end_date": null
        }))).await;
        assert_eq!(status, StatusCode::CREATED, "manager creates assignment: {body}");
        edges.push(body["id"].as_str().unwrap().to_string());
    }

    let first_path = format!("{assignment_path}/{}", edges[0]);
    let phases = [
        ("both active", None, false),
        ("ended first", Some((Method::PATCH, Some(json!({"end_date": "2026-09-17"})))), true),
        ("restored first", Some((Method::PATCH, Some(json!({"end_date": null})))), false),
        ("deleted first", Some((Method::DELETE, None)), true),
    ];
    for (phase, mutation, first_revoked) in phases {
        if let Some((method, payload)) = mutation {
            let (status, body) = rt.request_as(method, &first_path, manager, payload.as_ref()).await;
            assert_eq!(status, StatusCode::OK, "{phase}: {body}");
        }
        for (index, (token, _)) in readers.iter().enumerate() {
            let revoked = index == 0 && first_revoked;
            for entity in ["project", "document"] {
                let expected = if revoked {
                    vec![]
                } else if entity == "project" {
                    ids(&[shared.project])
                } else {
                    ids(&shared.documents)
                };
                let (status, body) = rt.request_as(Method::GET,
                    &format!("/api/v1/apps/{APP}/collections/{entity}"), token, None).await;
                assert_eq!(status, StatusCode::OK, "{phase}/reader {index}/{entity}: {body}");
                assert_eq!(http_ids(&body), expected,
                    "{phase}/reader {index}/{entity}: revocation must be individual");
                let (status, body) = rpc(&rt, APP, token, &select_ids(APP, entity), json!([]), true).await;
                assert_eq!(status, StatusCode::OK, "{phase}/reader {index}/{entity}: {body}");
                assert_eq!(sql_ids(&body["result"]), expected,
                    "{phase}/reader {index}/{entity}: an existing worker must observe individual revocation");
            }
        }
        let (status, second) = rt.request_as(Method::GET,
            &format!("{assignment_path}/{}", edges[1]), manager, None).await;
        assert_eq!(status, StatusCode::OK, "{phase}: second assignment must survive: {second}");
        assert!(second["end_date"].is_null(), "{phase}: second assignment must stay active");
    }
    rt.shutdown().await;
}

#[tokio::test]
async fn lifecycle_prunes_resource_targets_without_reinterpreting_legacy_or_mixed_shares() {
    let f = fixture(true).await;
    let mut historical = manifest(true);
    historical["appId"] = json!(LEGACY);
    contract_mut(&mut historical, "assignment")
        .as_object_mut()
        .unwrap()
        .remove("share");
    contract_mut(&mut historical, "legacy_assignment")["share"] = identity_share();
    f.rt.install_manifest(&historical).await;
    let helper = insert(&f.rt, LEGACY, "member", json!({"core_user_id": f.user})).await;
    let owner = insert(
        &f.rt,
        LEGACY,
        "member",
        json!({"core_user_id": f.owner_user}),
    )
    .await;
    let legacy_root = resource(&f.rt, LEGACY, Some(owner), "legacy").await;
    let legacy_sibling = resource(&f.rt, LEGACY, Some(owner), "legacy-sibling").await;
    insert(
        &f.rt,
        LEGACY,
        "legacy_assignment",
        json!({"member_id": helper, "project_id": legacy_root.project}),
    )
    .await;
    deploy(&f.rt, LEGACY).await;
    let historical_projection = projection(&f.rt, LEGACY).await;
    assert_eq!(
        historical_projection.0, 1,
        "omitted scope must retain the v1 contract"
    );
    let resource_projection = projection(&f.rt, APP).await;
    assert_eq!(
        resource_projection.0, 2,
        "resource paths require contract v2"
    );
    let mut keys = read_keys(APP, &["project", "document"]);
    keys.extend(read_keys(LEGACY, &["document"]));
    permissions(&f.rt, f.user, keys).await;

    for phase in [
        "initial",
        "redeploy",
        "rebuild",
        "prune",
        "bootstrap pruned",
        "restore",
        "mixed",
        "resource removed",
    ] {
        match phase {
            "redeploy" => {
                let mut updated = manifest(true);
                updated["version"] = json!("1.0.1");
                f.rt.install_manifest(&updated).await;
                deploy(&f.rt, APP).await;
            }
            "rebuild" => {
                // Remove only executable artifacts; bootstrap must reconstruct
                // from the durable v2 projection, including the multihop path.
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
                    "must remove real resource resolver artifacts"
                );
                for function in functions {
                    sqlx::query(&format!("DROP FUNCTION rootcx_system.{function}() CASCADE"))
                        .execute(f.rt.pool())
                        .await
                        .unwrap();
                }
                RbacExtension.bootstrap(f.rt.pool()).await.unwrap();
            }
            "prune" => {
                let mut pruned = manifest(true);
                contract_mut(&mut pruned, "assignment")["share"]
                    .as_object_mut()
                    .unwrap()
                    .remove("targets");
                f.rt.install_manifest(&pruned).await;
            }
            "bootstrap pruned" => RbacExtension.bootstrap(f.rt.pool()).await.unwrap(),
            "restore" => f.rt.install_manifest(&manifest(true)).await,
            "mixed" | "resource removed" => {
                let mut mixed = manifest(true);
                contract_mut(&mut mixed, "legacy_assignment")["share"] = identity_share();
                if phase == "resource removed" {
                    contract_mut(&mut mixed, "assignment")
                        .as_object_mut()
                        .unwrap()
                        .remove("share");
                }
                f.rt.install_manifest(&mixed).await;
                if phase == "mixed" {
                    // This identity branch grants self, while the resource branch
                    // grants a different owner's single project. Neither alone
                    // can produce the expected union; the sibling stays private.
                    insert(
                        &f.rt,
                        APP,
                        "legacy_assignment",
                        json!({"member_id": f.member, "project_id": f.own.project}),
                    )
                    .await;
                }
                RbacExtension.bootstrap(f.rt.pool()).await.unwrap();
            }
            _ => {}
        }
        assert_eq!(
            projection(&f.rt, APP).await.0,
            if phase == "resource removed" { 1 } else { 2 },
            "{phase}: contract version must describe the remaining sharing semantics"
        );
        let pruned = matches!(phase, "prune" | "bootstrap pruned");
        for entity in ["project", "document"] {
            let mut expected = if phase == "resource removed" || (pruned && entity == "document") {
                vec![]
            } else if entity == "project" {
                vec![f.shared.project]
            } else {
                f.shared.documents.clone()
            };
            if matches!(phase, "mixed" | "resource removed") {
                expected.extend(if entity == "project" {
                    vec![f.own.project]
                } else {
                    f.own.documents.clone()
                });
            }
            let (status, body) = rpc(
                &f.rt,
                APP,
                &f.token,
                &select_ids(APP, entity),
                json!([]),
                false,
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{phase}/{entity}: {body}");
            assert_eq!(
                sql_ids(&body["result"]),
                ids(&expected),
                "{phase}/{entity}: {body}"
            );
        }
        if pruned {
            assert_eq!(
                shared_keys(&f.rt, APP).await,
                vec![format!("app:{APP}:project.read.shared")],
                "{phase}: stale target permission"
            );
            let remaining: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace
                 WHERE n.nspname = 'rootcx_system' AND p.proname = $1)"
            ).bind(format!("rootcx_shared.{APP}.document")).fetch_one(f.rt.pool()).await.unwrap();
            assert!(
                !remaining,
                "{phase}: removed target resolver still callable"
            );
        } else if !matches!(phase, "mixed" | "resource removed") {
            let mut expected = vec![
                format!("app:{APP}:project.read.shared"),
                format!("app:{APP}:document.read.shared"),
            ];
            expected.sort();
            assert_eq!(shared_keys(&f.rt, APP).await, expected, "{phase}");
            assert_eq!(
                projection(&f.rt, APP).await,
                resource_projection,
                "{phase}: resource contract changed on reconstruction"
            );
        }
        let (_, body) = rpc(
            &f.rt,
            LEGACY,
            &f.token,
            &select_ids(LEGACY, "document"),
            json!([]),
            false,
        )
        .await;
        let expected: Vec<_> = legacy_root
            .documents
            .iter()
            .chain(&legacy_sibling.documents)
            .copied()
            .collect();
        assert_eq!(
            sql_ids(&body["result"]),
            ids(&expected),
            "{phase}: omitted scope must still share same-identity siblings"
        );
        assert_eq!(
            projection(&f.rt, LEGACY).await,
            historical_projection,
            "{phase}: bootstrap must not silently rewrite historical declarations"
        );
    }
    f.rt.shutdown().await;
}
