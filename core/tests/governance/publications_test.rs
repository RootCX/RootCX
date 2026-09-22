//! Public disclosure is a separate, live ceiling across HTTP, Bun IPC and SQL.
use crate::harness::{self, TestRuntime};
use futures::FutureExt;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use uuid::Uuid;

const RPC: &str = "/api/v1/apps/consumer/rpc";
const PUBLIC_COLLECTION: &str = "/api/v1/public/apps/consumer/collections/products";
const PUBLICATIONS: &str = "/api/v1/apps/consumer/publications";
const BACKEND: &[u8] = br#"
const instance = crypto.randomUUID();
async function read(params, _caller, ctx) {
  try {
    const c = params.remote
      ? ctx.remote("provider").collection("products")
      : ctx.collection("products");
    const result = params.op === "sql"
      ? await ctx.sql("SELECT * FROM consumer.products")
      : params.op === "job"
        ? await ctx.enqueueJob({kind: "publication-test"})
        : await c[params.op](...(params.args ?? []));
    return {result, instance};
  } catch (error) { return {error: error.message, instance}; }
}
serve({rpc: {local: read, remote: read, unbound: read}});
"#;

fn manifest(app: &str, owned: bool) -> Value {
    json!({
        "appId": app, "name": app, "version": "1.0.0",
        "dataContract": [{"entityName": "products", "fields": [
            {"name": "name", "type": "text"},
            {"name": "published", "type": "boolean"},
            {"name": "internal", "type": "text"},
            {"name": "secret", "type": "text", "sensitive": true},
            {"name": "user_id", "type": "uuid", "owner": owned}
        ]}]
    })
}

fn consumer_manifest(owned: bool) -> Value {
    let mut value = manifest("consumer", owned);
    value["public"] = json!({
        "publications": [
            {"name": "local_catalog", "entity": "products", "actions": ["list", "read"],
             "fields": ["name"], "where": {"published": true}},
            {"name": "remote_catalog", "app": "provider", "entity": "products",
             "actions": ["list", "read"], "fields": ["name"], "where": {"published": true}}
        ],
        "rpcs": [
            {"name": "local", "publications": ["local_catalog"]},
            {"name": "remote", "publications": ["remote_catalog"]},
            {"name": "unbound"}
        ],
        "collections": [
            {"entity": "products", "actions": ["list", "read"], "publication": "local_catalog"}
        ]
    });
    value
}

async fn deploy(rt: &TestRuntime) -> (StatusCode, Value) {
    rt.deploy("consumer", &harness::make_tar_gz(&[("index.ts", BACKEND)]))
        .await
}

async fn call(
    rt: &TestRuntime,
    method: &str,
    op: &str,
    args: Value,
    token: Option<&str>,
) -> (StatusCode, Value) {
    let body = json!({"method": method, "params": {
        "remote": method == "remote", "op": op, "args": args
    }});
    match token {
        Some(token) => rt.request_as(Method::POST, RPC, token, Some(&body)).await,
        None => rt.post_unauthed(RPC, &body).await,
    }
}

async fn public_get(rt: &TestRuntime, path: &str, query: &[(&str, &str)]) -> (StatusCode, Value) {
    let response = rt
        .client
        .get(rt.url(path))
        .query(query)
        .send()
        .await
        .unwrap();
    let status = response.status();
    (status, response.json().await.unwrap_or(Value::Null))
}

fn approval(declaration: &Value) -> Value {
    json!({
        "consumerInstallationId": declaration["consumerInstallationId"],
        "providerInstallationId": declaration["providerInstallationId"],
        "reason": "Publish the reviewed product catalog"
    })
}

async fn seed(rt: &TestRuntime, app: &str, owner: Uuid) -> Vec<Value> {
    let mut rows = Vec::new();
    for (name, published) in [("Alpha", true), ("Beta", true), ("Draft", false)] {
        rows.push(
            rt.create(
                app,
                "products",
                &json!({
                    "name": name, "published": published, "internal": "private",
                    "secret": "credential", "user_id": owner
                }),
            )
            .await,
        );
    }
    rows
}

async fn approve_declared(rt: &TestRuntime) {
    let declarations: Vec<Value> = rt.client.get(rt.url(PUBLICATIONS))
        .bearer_auth(&rt.token).send().await.unwrap().error_for_status().unwrap()
        .json().await.unwrap();
    for declaration in &declarations {
        let name = declaration["name"].as_str().unwrap();
        rt.client.post(rt.url(&format!("{PUBLICATIONS}/{name}/approve")))
            .bearer_auth(&rt.token).json(&approval(declaration)).send().await.unwrap()
            .error_for_status().unwrap();
    }
}

async fn approve_grant(rt: &TestRuntime, fields: Value) -> Value {
    let grant: Value = rt.client.post(rt.url("/api/v1/cross-app/grants"))
        .bearer_auth(&rt.token).json(&json!({
            "consumerApp": "consumer", "providerApp": "provider", "entity": "products",
            "actions": ["list", "read"], "fields": fields
        })).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
    rt.client.post(rt.url(&format!("/api/v1/cross-app/grants/{}/approve", grant["id"].as_str().unwrap())))
        .bearer_auth(&rt.token).json(&json!({})).send().await.unwrap().error_for_status().unwrap();
    grant
}

fn local_manifest(owned: bool) -> Value {
    let mut value = consumer_manifest(owned);
    value["public"]["publications"].as_array_mut().unwrap().truncate(1);
    value["public"]["rpcs"] = json!([{"name": "local", "publications": ["local_catalog"]}]);
    value
}

#[tokio::test]
async fn public_worker_queries_keep_the_publication_ceiling_for_anonymous_and_admin_callers() {
    let rt = TestRuntime::boot().await;
    let outcome = std::panic::AssertUnwindSafe(async {
        rt.install_manifest(&manifest("provider", false)).await;
        rt.install_manifest(&consumer_manifest(false)).await;
        let local_rows = seed(&rt, "consumer", Uuid::new_v4()).await;
        let remote_rows = seed(&rt, "provider", Uuid::new_v4()).await;
        let (s, body) = deploy(&rt).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        let (s, declarations) = rt.get_json(PUBLICATIONS).await;
        assert_eq!(s, StatusCode::OK, "{declarations}");
        let declarations = declarations.as_array().expect("declarations array");
        assert_eq!(declarations.len(), 2);
        for declaration in declarations {
            assert_eq!(declaration["status"], "pending", "{declaration}");
            assert!(declaration["consumerInstallationId"].is_string(), "{declaration}");
            assert!(declaration["providerInstallationId"].is_string(), "{declaration}");
        }
        for method in ["local", "remote"] {
            let (s, body) = call(&rt, method, "find", json!([]), None).await;
            assert_eq!(s, StatusCode::FORBIDDEN, "{method}: {body}");
        }

        for declaration in declarations {
            let name = declaration["name"].as_str().unwrap();
            let (s, body) = rt.post_json(
                &format!("{PUBLICATIONS}/{name}/approve"), &approval(declaration),
            ).await;
            assert_eq!(s, StatusCode::CREATED, "{name}: {body}");
        }
        let (s, active) = rt.get_json(PUBLICATIONS).await;
        assert_eq!(s, StatusCode::OK, "{active}");
        assert!(active.as_array().unwrap().iter().all(|d| d["status"] == "active"), "{active}");
        let (s, body) = call(&rt, "remote", "find", json!([]), None).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        assert!(body["error"].is_string(), "publication alone must not replace a grant: {body}");
        approve_grant(&rt, json!(["name", "published", "internal"])).await;

        for method in ["local", "remote"] {
            let existing_id = if method == "local" { &local_rows[0]["id"] } else { &remote_rows[0]["id"] };
            for token in [None, Some(rt.token.as_str())] {
                let context = format!("{method}, authenticated={}", token.is_some());
                let (s, body) = call(&rt, method, "find", json!([]), token).await;
                assert_eq!(s, StatusCode::OK, "{context}: {body}");
                let mut rows = body["result"].as_array()
                    .unwrap_or_else(|| panic!("{context}: expected find array, got {body}")).clone();
                rows.sort_by_key(|row| row["name"].as_str().unwrap().to_owned());
                assert_eq!(rows, vec![json!({"name": "Alpha"}), json!({"name": "Beta"})],
                    "{context}: a valid admin JWT must not broaden public rows or fields");
                for (offset, expected) in [(0, json!([{"name": "Alpha"}])), (2, json!([]))] {
                    let (s, body) = call(&rt, method, "findPage", json!([{
                        "limit": 1, "offset": offset, "orderBy": "name", "order": "asc"
                    }]), token).await;
                    assert_eq!(s, StatusCode::OK, "{context}: {body}");
                    assert_eq!(body["result"]["data"], expected, "{context}, offset={offset}: {body}");
                    assert_eq!(body["result"]["total"], 2, "{context}: {body}");
                }
                for (name, expected) in [("Alpha", json!({"name": "Alpha"})), ("Draft", Value::Null)] {
                    let (s, body) = call(&rt, method, "findOne", json!([{"name": name}]), token).await;
                    assert_eq!(s, StatusCode::OK, "{context}: {body}");
                    assert_eq!(body.get("result"), Some(&expected), "{context}/{name}: {body}");
                }
                for (op, args) in [
                    ("find", json!([{"internal": "private"}])),
                    ("find", json!([{"published": false}])),
                    ("findPage", json!([{"orderBy": "internal"}])),
                    ("create", json!([{"name": "injected"}])),
                    ("update", json!([existing_id, {"name": "injected"}])),
                    ("delete", json!([existing_id])),
                    ("sql", json!([])),
                    ("job", json!([])),
                ] {
                    let (s, body) = call(&rt, method, op, args.clone(), token).await;
                    assert_eq!(s, StatusCode::OK, "{context}/{op}: {body}");
                    assert!(body["error"].is_string(), "{context}/{op}/{args}: {body}");
                }
            }
        }
        let (s, body) = call(&rt, "unbound", "find", json!([]), None).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        assert_eq!(body["result"], json!([]), "legacy anonymous RPC retains its empty RLS scope: {body}");

    }).catch_unwind().await;
    rt.shutdown().await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
async fn provider_review_is_confined_to_its_own_publications() {
    let rt = TestRuntime::boot().await;
    let outcome = std::panic::AssertUnwindSafe(async {
        rt.install_manifest(&manifest("provider", false)).await;
        rt.install_manifest(&consumer_manifest(false)).await;
        let (s, declarations) = rt.get_json(PUBLICATIONS).await;
        assert_eq!(s, StatusCode::OK, "{declarations}");
        let declarations = declarations.as_array().unwrap();
        // A consumer's approval permission cannot approve the provider's disclosure.
        let token = rt.create_user("consumer-approver@test.local").await;
        let uid: Uuid = sqlx::query_scalar(
            "SELECT id FROM rootcx_system.users WHERE email = 'consumer-approver@test.local'",
        ).fetch_one(rt.pool()).await.unwrap();
        sqlx::query("DELETE FROM rootcx_system.rbac_assignments WHERE user_id = $1")
            .bind(uid).execute(rt.pool()).await.unwrap();
        let (s, body) = rt.post_json("/api/v1/roles", &json!({
            "name": "consumer_approver", "permissions": ["app:consumer:publications.approve"]
        })).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        sqlx::query("INSERT INTO rootcx_system.rbac_assignments (user_id, role) VALUES ($1, 'consumer_approver')")
            .bind(uid).execute(rt.pool()).await.unwrap();
        let remote = declarations.iter().find(|d| d["name"] == "remote_catalog").unwrap();
        let (s, body) = rt.request_as(Method::POST,
            &format!("{PUBLICATIONS}/remote_catalog/approve"), &token, Some(&approval(remote)),
        ).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{body}");
        // A provider reviewer can discover its own request in a mixed manifest.
        sqlx::query("UPDATE rootcx_system.rbac_roles SET permissions = $1 WHERE name = 'consumer_approver'")
            .bind(vec!["app:provider:publications.approve"])
            .execute(rt.pool()).await.unwrap();
        let (s, visible) = rt.request_as(Method::GET, PUBLICATIONS, &token, None).await;
        assert_eq!(s, StatusCode::OK, "{visible}");
        assert_eq!(visible.as_array().unwrap().len(), 1, "{visible}");
        assert_eq!(visible[0]["name"], "remote_catalog", "{visible}");
        let (s, body) = rt.request_as(Method::POST,
            &format!("{PUBLICATIONS}/remote_catalog/approve"), &token, Some(&approval(remote)),
        ).await;
        assert_eq!(s, StatusCode::CREATED, "{body}");
    }).catch_unwind().await;
    rt.shutdown().await;
    if let Err(panic) = outcome { std::panic::resume_unwind(panic); }
}

#[tokio::test]
async fn cached_public_workers_recheck_grant_and_publication_revocation() {
    let rt = TestRuntime::boot().await;
    let outcome = std::panic::AssertUnwindSafe(async {
        rt.install_manifest(&manifest("provider", false)).await;
        rt.install_manifest(&consumer_manifest(false)).await;
        seed(&rt, "provider", Uuid::new_v4()).await;
        let (s, body) = deploy(&rt).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        approve_declared(&rt).await;
        let grant = approve_grant(&rt, json!(["name", "published"])).await;
        let grant_path = format!("/api/v1/cross-app/grants/{}", grant["id"].as_str().unwrap());
        let (s, body) = call(&rt, "local", "find", json!([]), None).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        assert_eq!(body["result"], json!([]), "warm the local public worker: {body}");
        // Revoke without redeploying or stopping the already-used worker.
        let (s, warmed) = call(&rt, "remote", "find", json!([]), None).await;
        assert_eq!(s, StatusCode::OK, "{warmed}");
        assert_eq!(warmed["result"], json!([{"name": "Beta"}, {"name": "Alpha"}]), "{warmed}");
        let (s, body) = rt.post_json(&format!("{grant_path}/revoke"), &json!({})).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        let (s, denied) = call(&rt, "remote", "find", json!([]), None).await;
        assert_eq!(s, StatusCode::OK, "{denied}");
        assert_eq!(denied["instance"], warmed["instance"], "exercise the cached Bun worker");
        assert!(denied["error"].is_string(), "{denied}");
        let (s, body) = rt.post_json(&format!("{PUBLICATIONS}/local_catalog/revoke"),
            &json!({"reason": "Withdraw catalog"})).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        for token in [None, Some(rt.token.as_str())] {
            let (s, body) = call(&rt, "local", "find", json!([]), token).await;
            assert_eq!(s, StatusCode::FORBIDDEN, "{body}");
        }
        let (s, declarations) = rt.get_json(PUBLICATIONS).await;
        assert_eq!(s, StatusCode::OK, "{declarations}");
        let local = declarations.as_array().unwrap().iter()
            .find(|d| d["name"] == "local_catalog").unwrap();
        assert_eq!(local["status"], "revoked", "{local}");
    }).catch_unwind().await;
    rt.shutdown().await;
    if let Err(panic) = outcome { std::panic::resume_unwind(panic); }
}

#[tokio::test]
async fn public_reads_release_owned_rows_only_after_explicit_approval() {
    let rt = TestRuntime::boot().await;
    let outcome = std::panic::AssertUnwindSafe(async {
        let mut manifest = local_manifest(true);
        rt.install_manifest(&manifest).await;
        let owner: Uuid = sqlx::query_scalar(
            "SELECT id FROM rootcx_system.users WHERE email = 'admin@test.local'",
        )
        .fetch_one(rt.pool())
        .await
        .unwrap();
        seed(&rt, "consumer", owner).await;
        let (s, body) = deploy(&rt).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        let (s, body) = public_get(&rt, PUBLIC_COLLECTION, &[]).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{body}");
        for release in [false, true] {
            if release {
                manifest["public"]["publications"][0]["releaseOwnership"] = json!(true);
                rt.install_manifest(&manifest).await;
            }
            let (s, declarations) = rt.get_json(PUBLICATIONS).await;
            assert_eq!(s, StatusCode::OK, "{declarations}");
            let (s, body) = rt
                .post_json(
                    &format!("{PUBLICATIONS}/local_catalog/approve"),
                    &approval(&declarations[0]),
                )
                .await;
            assert_eq!(s, StatusCode::CREATED, "{body}");
            for token in [None, Some(rt.token.as_str())] {
                let (s, body) = call(&rt, "local", "find", json!([]), token).await;
                assert_eq!(s, StatusCode::OK, "{body}");
                let visible = body["result"].as_array().expect("owned public read");
                assert_eq!(
                    visible.len(),
                    if release { 2 } else { 0 },
                    "release={release}: never inherit the visiting human's ownership: {body}"
                );
                assert!(
                    visible
                        .iter()
                        .all(|row| row == &json!({"name": "Alpha"})
                            || row == &json!({"name": "Beta"})),
                    "{body}"
                );
            }
            let (s, body) = public_get(
                &rt,
                PUBLIC_COLLECTION,
                &[
                    ("limit", "1"),
                    ("offset", "0"),
                    ("orderBy", "name"),
                    ("order", "asc"),
                ],
            )
            .await;
            assert_eq!(s, StatusCode::OK, "{body}");
            assert_eq!(body["total"], if release { 2 } else { 0 }, "{body}");
            assert_eq!(
                body["data"],
                if release {
                    json!([{"name": "Alpha"}])
                } else {
                    json!([])
                },
                "{body}"
            );
        }
    })
    .catch_unwind()
    .await;
    rt.shutdown().await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
async fn installation_rotation_requires_fresh_approval_ids() {
    let rt = TestRuntime::boot().await;
    let outcome = std::panic::AssertUnwindSafe(async {
        let mut manifest = local_manifest(false);
        rt.install_manifest(&manifest).await;
        seed(&rt, "consumer", Uuid::new_v4()).await;
        let (s, body) = deploy(&rt).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        let (s, declarations) = rt.get_json(PUBLICATIONS).await;
        assert_eq!(s, StatusCode::OK, "{declarations}");
        let saved = approval(&declarations[0]);
        let path = format!("{PUBLICATIONS}/local_catalog/approve");
        let (s, body) = rt.post_json(&path, &saved).await;
        assert_eq!(s, StatusCode::CREATED, "{body}");
        let (s, body) = call(&rt, "local", "find", json!([]), None).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        assert_eq!(body["result"].as_array().unwrap().len(), 2, "{body}");

        manifest["dataContract"][0]["fields"]
            .as_array_mut()
            .unwrap()
            .push(json!({"name": "new_private_field", "type": "text"}));
        rt.install_manifest(&manifest).await;
        let (s, declarations) = rt.get_json(PUBLICATIONS).await;
        assert_eq!(s, StatusCode::OK, "{declarations}");
        assert_ne!(
            declarations[0]["consumerInstallationId"],
            saved["consumerInstallationId"]
        );
        assert_ne!(
            declarations[0]["providerInstallationId"],
            saved["providerInstallationId"]
        );
        assert_eq!(declarations[0]["status"], "pending", "{declarations}");
        let (s, body) = rt.post_json(&path, &saved).await;
        assert_eq!(s, StatusCode::CONFLICT, "{body}");
        let (s, body) = call(&rt, "local", "find", json!([]), None).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{body}");
        let (s, body) = rt.post_json(&path, &approval(&declarations[0])).await;
        assert_eq!(s, StatusCode::CREATED, "{body}");
        let (s, body) = call(&rt, "local", "findOne", json!([{"name": "Alpha"}]), None).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        assert_eq!(body["result"], json!({"name": "Alpha"}), "{body}");
    })
    .catch_unwind()
    .await;
    rt.shutdown().await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
async fn direct_public_routes_enforce_projection_predicates_and_read_only_access() {
    let rt = TestRuntime::boot().await;
    let outcome = std::panic::AssertUnwindSafe(async {
        rt.install_manifest(&local_manifest(false)).await;
        let rows = seed(&rt, "consumer", Uuid::new_v4()).await;
        approve_declared(&rt).await;
        let (s, body) =
            public_get(&rt, PUBLIC_COLLECTION, &[("where", r#"{"name":"Beta"}"#)]).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        assert_eq!(body["data"], json!([{"name": "Beta"}]), "{body}");
        assert_eq!(body["total"], 1, "{body}");
        for (row, expected) in [
            (&rows[0], StatusCode::OK),
            (&rows[2], StatusCode::NOT_FOUND),
        ] {
            let (s, body) = public_get(
                &rt,
                &format!("{PUBLIC_COLLECTION}/{}", row["id"].as_str().unwrap()),
                &[],
            )
            .await;
            assert_eq!(s, expected, "{body}");
            if s == StatusCode::OK {
                assert_eq!(body, json!({"name": "Alpha"}));
            }
        }
        for (query, expected) in [
            (vec![("where", r#"{"internal":"private"}"#)], StatusCode::FORBIDDEN),
            (vec![("orderBy", "internal")], StatusCode::FORBIDDEN),
            (vec![("where", "{")], StatusCode::BAD_REQUEST),
        ] {
            let (s, body) = public_get(&rt, PUBLIC_COLLECTION, &query).await;
            assert_eq!(s, expected, "{query:?}: {body}");
        }
        for (method, path) in [
            (Method::POST, PUBLIC_COLLECTION.to_owned()),
            (
                Method::PATCH,
                format!("{PUBLIC_COLLECTION}/{}", rows[0]["id"].as_str().unwrap()),
            ),
            (
                Method::DELETE,
                format!("{PUBLIC_COLLECTION}/{}", rows[0]["id"].as_str().unwrap()),
            ),
        ] {
            let response = rt
                .client
                .request(method.clone(), rt.url(&path))
                .json(&json!({"name": "injected"}))
                .send()
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::METHOD_NOT_ALLOWED,
                "{method}"
            );
        }
        let (s, stored) = rt
            .get_json("/api/v1/apps/consumer/collections/products")
            .await;
        assert_eq!(s, StatusCode::OK, "{stored}");
        assert_eq!(stored.as_array().unwrap().len(), 3, "{stored}");
        assert!(!stored.to_string().contains("injected"), "{stored}");
    }).catch_unwind().await;
    rt.shutdown().await;
    if let Err(panic) = outcome { std::panic::resume_unwind(panic); }
}

#[tokio::test]
async fn remote_public_projection_is_the_intersection_with_the_live_grant() {
    let rt = TestRuntime::boot().await;
    let outcome = std::panic::AssertUnwindSafe(async {
        rt.install_manifest(&manifest("provider", false)).await;
        let mut consumer = local_manifest(false);
        consumer["public"]["publications"][0]["app"] = json!("provider");
        consumer["public"]["publications"][0]["fields"] = json!(["name", "internal"]);
        rt.install_manifest(&consumer).await;
        seed(&rt, "provider", Uuid::new_v4()).await;
        approve_declared(&rt).await;
        for (fields, expected_status) in [
            (json!(["name", "published"]), StatusCode::OK),
            (json!(["published"]), StatusCode::FORBIDDEN),
        ] {
            let grant = approve_grant(&rt, fields.clone()).await;
            let (s, body) = public_get(&rt, PUBLIC_COLLECTION, &[]).await;
            assert_eq!(s, expected_status, "grant fields={fields}: {body}");
            if s == StatusCode::OK {
                assert_eq!(body["data"], json!([{"name": "Beta"}, {"name": "Alpha"}]), "{body}");
                assert_eq!(body["total"], 2, "{body}");
                let (s, body) = public_get(&rt, PUBLIC_COLLECTION, &[("where", r#"{"internal":"private"}"#)]).await;
                assert_eq!(s, StatusCode::FORBIDDEN, "publication cannot authorize a filter outside the grant: {body}");
            }
            let (s, body) = rt.post_json(
                &format!("/api/v1/cross-app/grants/{}/revoke", grant["id"].as_str().unwrap()), &json!({}),
            ).await;
            assert_eq!(s, StatusCode::OK, "{body}");
        }
    }).catch_unwind().await;
    rt.shutdown().await;
    if let Err(panic) = outcome { std::panic::resume_unwind(panic); }
}

#[tokio::test]
async fn ownership_release_preserves_provider_restrictive_rls_for_rows_and_counts() {
    let rt = TestRuntime::boot().await;
    let outcome = std::panic::AssertUnwindSafe(async {
        rt.install_manifest(&manifest("provider", true)).await;
        let mut consumer = local_manifest(false);
        consumer["public"]["publications"][0]["app"] = json!("provider");
        consumer["public"]["publications"][0]["releaseOwnership"] = json!(true);
        rt.install_manifest(&consumer).await;
        let rows = seed(&rt, "provider", Uuid::new_v4()).await;
        approve_declared(&rt).await;
        approve_grant(&rt, json!(["id", "name", "published"])).await;
        let (s, body) = public_get(&rt, PUBLIC_COLLECTION, &[]).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        assert_eq!(body["total"], 2, "ownership release must be exercised: {body}");
        sqlx::query("CREATE POLICY provider_visibility ON provider.products AS RESTRICTIVE FOR SELECT USING (name <> 'Beta')")
            .execute(rt.pool()).await.unwrap();
        for (offset, expected) in [("0", json!([{"name": "Alpha"}])), ("1", json!([]))] {
            let (s, body) = public_get(&rt, PUBLIC_COLLECTION, &[("limit", "1"), ("offset", offset)]).await;
            assert_eq!(s, StatusCode::OK, "offset={offset}: {body}");
            assert_eq!(body["data"], expected, "offset={offset}: {body}");
            assert_eq!(body["total"], 1, "RLS must also constrain counts: {body}");
        }
        let (s, body) = public_get(&rt,
            &format!("{PUBLIC_COLLECTION}/{}", rows[1]["id"].as_str().unwrap()), &[],
        ).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "read-by-id must preserve provider RLS: {body}");
    }).catch_unwind().await;
    rt.shutdown().await;
    if let Err(panic) = outcome { std::panic::resume_unwind(panic); }
}

#[tokio::test]
async fn public_read_audit_correlates_outcomes_with_the_authority_actually_used() {
    let rt = TestRuntime::boot().await;
    let outcome = std::panic::AssertUnwindSafe(async {
        rt.install_manifest(&manifest("provider", false)).await;
        let mut consumer = local_manifest(false);
        consumer["public"]["publications"][0]["app"] = json!("provider");
        consumer["public"]["publications"][0]["fields"] = json!(["name", "internal"]);
        rt.install_manifest(&consumer).await;
        seed(&rt, "provider", Uuid::new_v4()).await;
        approve_declared(&rt).await;
        let grant = approve_grant(&rt, json!(["name", "published"])).await;
        let (s, body) = public_get(&rt, PUBLIC_COLLECTION, &[("limit", "1")]).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        assert_eq!(body["data"], json!([{"name": "Beta"}]), "{body}");
        let (s, body) = public_get(&rt, PUBLIC_COLLECTION, &[("where", r#"{"secret":"credential"}"#)]).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{body}");
        let events: Vec<Value> = sqlx::query_scalar(
            "SELECT to_jsonb(a) FROM rootcx_system.publication_read_audit a ORDER BY created_at, outcome",
        ).fetch_all(rt.pool()).await.unwrap();
        assert_eq!(events.len(), 4, "{events:?}");
        let success = events.iter().find(|e| e["outcome"] == "success").unwrap();
        let denied = events.iter().find(|e| e["outcome"] == "denied").unwrap();
        assert_ne!(success["correlation_id"], denied["correlation_id"]);
        for terminal in [success, denied] {
            let started: Vec<_> = events.iter().filter(|e|
                e["outcome"] == "started" && e["correlation_id"] == terminal["correlation_id"]
            ).collect();
            assert_eq!(started.len(), 1, "{terminal}");
            assert_eq!(started[0]["publication_id"], terminal["publication_id"], "{terminal}");
            assert_eq!(started[0]["principal_id"], terminal["principal_id"], "{terminal}");
        }
        let authority: (Uuid, i64, Uuid) = sqlx::query_as(
            "SELECT p.id, p.version, i.user_id FROM rootcx_system.publications p
             JOIN rootcx_system.public_execution_principals i ON i.installation_id = p.consumer_installation_id
             WHERE p.consumer_app = 'consumer' AND p.status = 'active'",
        ).fetch_one(rt.pool()).await.unwrap();
        assert_eq!(success["publication_id"], json!(authority.0));
        assert_eq!(success["publication_version"], authority.1);
        assert_eq!(success["principal_id"], json!(authority.2));
        assert_eq!(success["consumer_app"], "consumer");
        assert_eq!(success["provider_app"], "provider");
        assert_eq!(success["grant_id"], grant["id"]);
        let grant_version: i64 = sqlx::query_scalar(
            "SELECT version FROM rootcx_system.cross_app_collection_grants WHERE id = $1",
        ).bind(grant["id"].as_str().unwrap().parse::<Uuid>().unwrap())
            .fetch_one(rt.pool()).await.unwrap();
        assert_eq!(success["grant_version"], grant_version, "{success}");
        assert_eq!(success["projection"], json!(["name"]), "audit the effective intersection");
        assert_eq!(success["row_count"], 1, "audit the emitted page, not the total");
        assert!(denied["row_count"].is_null(), "{denied}");
    }).catch_unwind().await;
    rt.shutdown().await;
    if let Err(panic) = outcome { std::panic::resume_unwind(panic); }
}
