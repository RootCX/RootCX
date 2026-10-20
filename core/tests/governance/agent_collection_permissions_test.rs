use crate::harness::{self, TestRuntime};
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::time::Duration;
use uuid::Uuid;

// Exercise real admission, IPC tool dispatch and PostgreSQL RLS without a model.
const BACKEND: &[u8] = br#"
import { createInterface } from "node:readline";
const send = message => process.stdout.write(JSON.stringify(message) + "\n");
createInterface({input:process.stdin}).on("line", line => {
  const m = JSON.parse(line);
  if (m.type === "discover") send({type:"discover",protocol:2});
  if (m.type === "agent_invoke")
    send({type:"agent_tool_call",invoke_id:m.invoke_id,call_id:crypto.randomUUID(),...JSON.parse(m.message)});
  if (m.type === "agent_tool_result")
    send({type:"agent_done",invoke_id:m.invoke_id,response:JSON.stringify({result:m.result,error:m.error}),tokens:0});
});
"#;
const CONFIG: &[u8] = br#"{"name":"Collection helper","supervision":{"mode":"autonomous"}}"#;

fn archive() -> Vec<u8> {
    harness::make_tar_gz(&[("index.ts", BACKEND), ("agent.json", CONFIG)])
}

async fn setup() -> TestRuntime {
    let rt = TestRuntime::boot().await;
    rt.install_manifest(&json!({
        "appId":"business", "name":"Business", "version":"1.0.0",
        "dataContract":[
            {"entityName":"projects","fields":[
                {"name":"name","type":"text","required":true},
                {"name":"status","type":"text","default_value":"draft"},
                {"name":"secret","type":"text","sensitive":true}
            ]},
            {"entityName":"owned","fields":[
                {"name":"user_id","type":"uuid","owner":true},
                {"name":"name","type":"text","required":true}
            ]}
        ]
    }))
    .await;
    rt.install("helper", "notes").await;
    let (status, body) = rt.deploy("helper", &archive()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    rt
}

async fn agent_permissions(rt: &TestRuntime, permissions: &[&str]) {
    let (status, body) = rt
        .patch_json(
            "/api/v1/roles/app:helper:agent",
            &json!({
                "permissions":permissions
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

async fn human(rt: &TestRuntime, permissions: &[&str]) -> (String, Uuid) {
    let token = rt.create_user("limited@test.local").await;
    let uid: Uuid =
        sqlx::query_scalar("SELECT id FROM rootcx_system.users WHERE email='limited@test.local'")
            .fetch_one(rt.pool())
            .await
            .unwrap();
    sqlx::query("DELETE FROM rootcx_system.rbac_assignments WHERE user_id=$1")
        .bind(uid)
        .execute(rt.pool())
        .await
        .unwrap();
    let (status, body) = rt
        .post_json(
            "/api/v1/roles",
            &json!({
                "name":"limited","permissions":permissions
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = rt
        .post_json(
            "/api/v1/roles/assign",
            &json!({"userId":uid,"role":"limited"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    (token, uid)
}

async fn invoke(rt: &TestRuntime, token: &str, tool: &str, args: Value) -> Value {
    let response = rt
        .client
        .post(rt.url("/api/v1/apps/helper/agent/invoke"))
        .bearer_auth(token)
        .json(&json!({"message":json!({"tool_name":tool,"args":args}).to_string()}))
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body = response.text().await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    let done = body
        .split("\n\n")
        .find(|event| event.lines().any(|line| line.trim() == "event: done"))
        .unwrap_or_else(|| panic!("missing result: {body}"));
    let data: Value = serde_json::from_str(
        done.lines()
            .find_map(|line| line.strip_prefix("data:"))
            .unwrap(),
    )
    .unwrap();
    serde_json::from_str(data["response"].as_str().unwrap()).unwrap()
}

#[tokio::test]
async fn ordinary_agent_app_role_authorizes_crud_without_collection_grants() {
    let rt = setup().await;
    agent_permissions(&rt, &["app:helper:invoke", "app:business:*", "tool:*"]).await;
    let agent = rootcx_core::extensions::agents::agent_user_id("helper");
    let responsible_human: Uuid =
        sqlx::query_scalar("SELECT id FROM rootcx_system.users WHERE email='admin@test.local'")
            .fetch_one(rt.pool())
            .await
            .unwrap();
    let (roles, permissions) =
        rootcx_core::governance::authority::resolve_permissions(rt.pool(), agent)
            .await
            .unwrap();
    assert!(!roles.iter().any(|role| role == "admin"), "{roles:?}");
    assert!(
        !permissions.iter().any(|permission| permission == "*"),
        "{permissions:?}"
    );
    let created = invoke(&rt, &rt.token, "mutate_data", json!({
        "app":"business","entity":"projects","action":"create","data":{"name":"Synthetic project"}
    })).await;
    assert!(created["error"].is_null(), "{created}");
    let id = created["result"]["id"].as_str().unwrap();
    let read = invoke(
        &rt,
        &rt.token,
        "query_data",
        json!({"app":"business","entity":"projects"}),
    )
    .await;
    assert_eq!(read["result"][0]["id"], id, "{read}");
    let updated = invoke(&rt, &rt.token, "mutate_data", json!({
        "app":"business","entity":"projects","action":"update","id":id,"data":{"status":"active"}
    })).await;
    assert_eq!(updated["result"]["status"], "active", "{updated}");
    let deleted = invoke(
        &rt,
        &rt.token,
        "mutate_data",
        json!({
            "app":"business","entity":"projects","action":"delete","id":id
        }),
    )
    .await;
    assert!(deleted["error"].is_null(), "{deleted}");
    let (status, journal) = rt.get_json("/api/v1/cross-app/audit").await;
    assert_eq!(status, StatusCode::OK, "{journal}");
    let entries: Vec<_> = journal
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["consumerApp"] == "helper" && entry["providerApp"] == "business")
        .collect();
    assert_eq!(entries.len(), 8, "{journal}");
    assert!(
        entries
            .iter()
            .all(|entry| entry["authorizationKind"] == "agent_delegation"
                && entry["grantId"].is_null()),
        "{journal}"
    );
    let grants: i64 =
        sqlx::query_scalar("SELECT count(*) FROM rootcx_system.cross_app_collection_grants")
            .fetch_one(rt.pool())
            .await
            .unwrap();
    assert_eq!(grants, 0);
    let audit: Vec<(String, Option<Uuid>, Option<Uuid>)> = sqlx::query_as(
        "SELECT operation,actor_uid,delegator_uid FROM rootcx_system.audit_log
         WHERE table_schema='business' AND table_name='projects' ORDER BY id",
    )
    .fetch_all(rt.pool())
    .await
    .unwrap();
    assert_eq!(audit.len(), 3, "{audit:?}");
    for (operation, actor, human) in &audit {
        assert_eq!(*actor, Some(agent), "{operation}");
        assert_eq!(*human, Some(responsible_human), "{operation}");
    }
    let successful: Vec<_> = entries
        .iter()
        .filter(|entry| entry["outcome"] == "success")
        .collect();
    assert_eq!(successful.len(), 4, "{journal}");
    for entry in successful {
        assert_eq!(entry["principalId"], json!(agent), "{entry}");
        assert_eq!(
            entry["responsibleHumanId"],
            json!(responsible_human),
            "{entry}"
        );
    }
    rt.shutdown().await;
}

#[tokio::test]
async fn exact_mutation_permissions_do_not_grant_standalone_reads_or_other_operations() {
    let rt = setup().await;
    let mut id = Value::Null;
    for action in ["create", "update", "delete"] {
        let permission = format!("app:business:projects.{action}");
        agent_permissions(&rt, &["app:helper:invoke", "tool:*", &permission]).await;
        let result = invoke(
            &rt,
            &rt.token,
            "mutate_data",
            json!({
                "app":"business","entity":"projects","action":action,"id":id,"data":{"name":action}
            }),
        )
        .await;
        assert!(result["error"].is_null(), "{action}: {result}");
        if action == "create" {
            id = result["result"]["id"].clone();
        }
        let read = invoke(
            &rt,
            &rt.token,
            "query_data",
            json!({"app":"business","entity":"projects"}),
        )
        .await;
        assert!(
            read["error"].is_string(),
            "{action} must not grant reads: {read}"
        );
        let wrong_action = if action == "create" {
            "update"
        } else {
            "create"
        };
        let denied = invoke(&rt, &rt.token, "mutate_data", json!({
            "app":"business","entity":"projects","action":wrong_action,"id":id,"data":{"name":"denied"}
        })).await;
        assert!(
            denied["error"].is_string(),
            "{action} allowed {wrong_action}: {denied}"
        );
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM business.projects")
        .fetch_one(rt.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
    rt.shutdown().await;
}

#[tokio::test]
async fn metadata_and_sensitive_fields_follow_the_effective_collection_scope() {
    let rt = setup().await;
    agent_permissions(
        &rt,
        &["app:helper:invoke", "app:business:projects.read", "tool:*"],
    )
    .await;
    sqlx::query("INSERT INTO business.projects (name,secret) VALUES ('visible','classified')")
        .execute(rt.pool())
        .await
        .unwrap();
    let apps = invoke(&rt, &rt.token, "list_apps", json!({})).await;
    assert_eq!(
        apps["result"],
        json!([{"id":"business","name":"Business","entities":["projects"]}]),
        "{apps}"
    );
    let metadata = invoke(&rt, &rt.token, "describe_app", json!({"app":"business"})).await;
    assert_eq!(
        metadata["result"]["dataContract"],
        json!([{
            "entityName":"projects","fields":[{"name":"name","type":"text"},{"name":"status","type":"text"}]
        }]),
        "{metadata}"
    );
    let own = invoke(&rt, &rt.token, "describe_app", json!({"app":"helper"})).await;
    assert!(
        own["error"].is_string(),
        "own app must not expose unauthorized schema: {own}"
    );
    let hidden = invoke(
        &rt,
        &rt.token,
        "query_data",
        json!({"app":"business","entity":"owned"}),
    )
    .await;
    assert!(hidden["error"].is_string(), "{hidden}");
    let read = invoke(
        &rt,
        &rt.token,
        "query_data",
        json!({"app":"business","entity":"projects"}),
    )
    .await;
    assert_eq!(read["result"][0]["name"], "visible", "{read}");
    assert!(read["result"][0].get("secret").is_none(), "{read}");
    for extra in [
        json!({"where":{"secret":"classified"}}),
        json!({"orderBy":"secret"}),
    ] {
        let mut args = json!({"app":"business","entity":"projects"});
        args.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let denied = invoke(&rt, &rt.token, "query_data", args.clone()).await;
        assert!(
            denied["error"].is_string(),
            "sensitive query {args}: {denied}"
        );
    }
    rt.shutdown().await;
}

#[tokio::test]
async fn inactive_installations_hide_agent_metadata_and_refuse_collection_reads() {
    let rt = setup().await;
    agent_permissions(&rt, &["app:helper:invoke", "app:business:*", "tool:*"]).await;
    sqlx::query("UPDATE rootcx_system.app_installations SET active=FALSE WHERE app_id='business' AND active")
        .execute(rt.pool()).await.unwrap();
    let hidden = invoke(&rt, &rt.token, "list_apps", json!({})).await;
    assert_eq!(hidden["result"], json!([]), "{hidden}");
    for (tool, args) in [
        ("describe_app", json!({"app":"business"})),
        ("query_data", json!({"app":"business","entity":"projects"})),
    ] {
        let denied = invoke(&rt, &rt.token, tool, args).await;
        assert!(denied["error"].is_string(), "{tool}: {denied}");
    }
    rt.shutdown().await;
}

#[tokio::test]
async fn delegated_writes_validate_sensitive_fields_and_roll_back_invalid_batches() {
    let rt = setup().await;
    agent_permissions(&rt, &["app:helper:invoke", "app:business:*", "tool:*"]).await;
    for data in [
        Value::Null,
        json!({}),
        json!("not a record"),
        json!({"secret":"forbidden"}),
        json!({"missing":"forbidden"}),
        json!({"id":Uuid::new_v4()}),
    ] {
        let denied = invoke(
            &rt,
            &rt.token,
            "mutate_data",
            json!({
                "app":"business","entity":"projects","action":"create","data":data
            }),
        )
        .await;
        assert!(denied["error"].is_string(), "invalid {data}: {denied}");
    }
    for invalid in [
        json!({"name":"second","secret":"forbidden"}),
        json!({"status":"missing required name"}),
    ] {
        let result = invoke(&rt, &rt.token, "mutate_data", json!({
            "app":"business","entity":"projects","action":"bulk_create","data":[{"name":"first"},invalid]
        })).await;
        assert!(
            result["error"].is_string(),
            "invalid batch {invalid}: {result}"
        );
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM business.projects")
            .fetch_one(rt.pool())
            .await
            .unwrap();
        assert_eq!(count, 0, "partial batch persisted: {invalid}");
    }
    rt.shutdown().await;
}

#[tokio::test]
async fn responsible_human_permissions_and_ownership_remain_ceiling() {
    let rt = setup().await;
    agent_permissions(&rt, &["app:helper:invoke", "app:business:*", "tool:*"]).await;
    let (token, uid) = human(
        &rt,
        &[
            "app:helper:invoke",
            "tool:*",
            "app:business:owned.read.own",
            "app:business:owned.update.own",
        ],
    )
    .await;
    let other = Uuid::new_v4();
    let mine: Uuid = sqlx::query_scalar(
        "INSERT INTO business.owned (user_id,name) VALUES ($1,'mine') RETURNING id",
    )
    .bind(uid)
    .fetch_one(rt.pool())
    .await
    .unwrap();
    let foreign: Uuid = sqlx::query_scalar(
        "INSERT INTO business.owned (user_id,name) VALUES ($1,'foreign') RETURNING id",
    )
    .bind(other)
    .fetch_one(rt.pool())
    .await
    .unwrap();
    let read = invoke(
        &rt,
        &token,
        "query_data",
        json!({"app":"business","entity":"owned"}),
    )
    .await;
    assert_eq!(read["result"].as_array().unwrap().len(), 1, "{read}");
    assert_eq!(read["result"][0]["id"], mine.to_string(), "{read}");
    let updated = invoke(
        &rt,
        &token,
        "mutate_data",
        json!({
            "app":"business","entity":"owned","action":"update","id":mine,"data":{"name":"updated"}
        }),
    )
    .await;
    assert!(updated["error"].is_null(), "{updated}");
    for (id, data) in [
        (foreign, json!({"name":"stolen"})),
        (mine, json!({"user_id":other})),
    ] {
        let denied = invoke(
            &rt,
            &token,
            "mutate_data",
            json!({
                "app":"business","entity":"owned","action":"update","id":id,"data":data
            }),
        )
        .await;
        assert!(
            denied["error"].is_string(),
            "ownership {id}/{data}: {denied}"
        );
    }
    let denied = invoke(&rt, &token, "mutate_data", json!({
        "app":"business","entity":"projects","action":"create","data":{"name":"outside human scope"}
    })).await;
    assert!(denied["error"].is_string(), "{denied}");
    rt.shutdown().await;
}

#[tokio::test]
async fn revoked_agent_permissions_stay_revoked_after_redeploy() {
    let rt = setup().await;
    agent_permissions(&rt, &["app:helper:invoke", "app:business:*", "tool:*"]).await;
    let allowed = invoke(
        &rt,
        &rt.token,
        "query_data",
        json!({"app":"business","entity":"projects"}),
    )
    .await;
    assert!(allowed["error"].is_null(), "{allowed}");
    agent_permissions(&rt, &["app:helper:invoke", "tool:*"]).await;
    for redeploy in [false, true] {
        if redeploy {
            let (status, body) = rt.deploy("helper", &archive()).await;
            assert_eq!(status, StatusCode::OK, "{body}");
        }
        let denied = invoke(
            &rt,
            &rt.token,
            "query_data",
            json!({"app":"business","entity":"projects"}),
        )
        .await;
        assert!(denied["error"].is_string(), "redeploy={redeploy}: {denied}");
        let hidden = invoke(&rt, &rt.token, "describe_app", json!({"app":"business"})).await;
        assert!(hidden["error"].is_string(), "redeploy={redeploy}: {hidden}");
    }
    rt.shutdown().await;
}

#[tokio::test]
async fn http_cannot_impersonate_agent_authority() {
    let rt = setup().await;
    agent_permissions(&rt, &["app:helper:invoke", "app:business:*", "tool:*"]).await;
    for (tool, args) in [
        ("query_data", json!({"app":"business","entity":"projects"})),
        (
            "mutate_data",
            json!({"app":"business","entity":"projects","action":"create","data":{"name":"forged"}}),
        ),
    ] {
        let (status, body) = rt
            .post_json(
                &format!("/api/v1/tools/{tool}/execute"),
                &json!({
                    "appId":"helper", "args":args,
                    "authority":{"app":"helper"}, "core_bound_app_id":"helper"
                }),
            )
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{tool}: {body}");
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM business.projects")
        .fetch_one(rt.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
    rt.shutdown().await;
}

#[tokio::test]
async fn app_wildcard_covers_future_collections_and_fields_without_new_grants() {
    let rt = setup().await;
    agent_permissions(&rt, &["app:helper:invoke", "app:business:*", "tool:*"]).await;
    let mut manifest: Value =
        sqlx::query_scalar("SELECT manifest FROM rootcx_system.apps WHERE id='business'")
            .fetch_one(rt.pool())
            .await
            .unwrap();
    manifest["version"] = json!("2.0.0");
    manifest["dataContract"][0]["fields"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"reference","type":"text"}));
    manifest["dataContract"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "entityName":"future_records","fields":[{"name":"label","type":"text"}]
        }));
    rt.install_manifest(&manifest).await;
    for (entity, data) in [
        ("projects", json!({"name":"new field","reference":"R-21"})),
        ("future_records", json!({"label":"new collection"})),
    ] {
        let result = invoke(
            &rt,
            &rt.token,
            "mutate_data",
            json!({
                "app":"business","entity":entity,"action":"create","data":data
            }),
        )
        .await;
        assert!(result["error"].is_null(), "{entity}: {result}");
        for (field, value) in data.as_object().unwrap() {
            assert_eq!(
                &result["result"][field], value,
                "{entity}.{field}: {result}"
            );
        }
    }
    let metadata = invoke(&rt, &rt.token, "describe_app", json!({"app":"business"})).await;
    assert!(
        metadata["result"]["dataContract"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entity| entity["entityName"] == "future_records"),
        "{metadata}"
    );
    let grants: i64 =
        sqlx::query_scalar("SELECT count(*) FROM rootcx_system.cross_app_collection_grants")
            .fetch_one(rt.pool())
            .await
            .unwrap();
    assert_eq!(grants, 0);
    rt.shutdown().await;
}

#[tokio::test]
async fn successful_write_audit_failure_rolls_back_the_delegated_mutation() {
    let rt = setup().await;
    agent_permissions(&rt, &["app:helper:invoke", "app:business:*", "tool:*"]).await;
    sqlx::raw_sql(
        "CREATE FUNCTION rootcx_system.reject_agent_audit() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN
           IF NEW.outcome='success' AND NEW.consumer_app='helper' THEN
             RAISE EXCEPTION 'injected agent audit outage';
           END IF;
           RETURN NEW;
         END $$;
         CREATE TRIGGER reject_agent_audit BEFORE INSERT ON rootcx_system.cross_app_read_audit
         FOR EACH ROW EXECUTE FUNCTION rootcx_system.reject_agent_audit();",
    )
    .execute(rt.pool())
    .await
    .unwrap();
    let failed = invoke(
        &rt,
        &rt.token,
        "mutate_data",
        json!({
            "app":"business","entity":"projects","action":"create","data":{"name":"must roll back"}
        }),
    )
    .await;
    assert!(failed["error"].is_string(), "{failed}");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM business.projects")
        .fetch_one(rt.pool())
        .await
        .unwrap();
    assert_eq!(count, 0, "audit failure left business data committed");
    let audit: i64 = sqlx::query_scalar("SELECT count(*) FROM rootcx_system.audit_log WHERE table_schema='business' AND table_name='projects'")
        .fetch_one(rt.pool()).await.unwrap();
    assert_eq!(
        audit, 0,
        "rolled-back business write left a successful write audit"
    );
    rt.shutdown().await;
}

#[tokio::test]
async fn shared_read_permission_obeys_live_resource_assignments_and_never_grants_writes() {
    let rt = setup().await;
    rt.install_manifest(&json!({
        "appId":"shared_space","name":"Shared space","version":"1.0.0",
        "dataContract":[
            {"entityName":"people","fields":[
                {"name":"user_id","type":"entity_link","owner":true,"references":{"entity":"core:users","field":"id"}}
            ]},
            {"entityName":"projects","fields":[{"name":"name","type":"text"}]},
            {"entityName":"assignments","share":{
                "scope":"resource","grantee":"person_id","subject":"project_id","activeWhen":{"isNull":"ended_at"}
            },"fields":[
                {"name":"person_id","type":"entity_link","references":{"entity":"people","field":"id"}},
                {"name":"project_id","type":"entity_link","references":{"entity":"projects","field":"id"}},
                {"name":"ended_at","type":"date"}
            ]}
        ]
    })).await;
    agent_permissions(&rt, &["app:helper:invoke", "app:shared_space:*", "tool:*"]).await;
    let (token, uid) = human(
        &rt,
        &[
            "app:helper:invoke",
            "tool:*",
            "app:shared_space:projects.read.shared",
        ],
    )
    .await;
    let person: Uuid =
        sqlx::query_scalar("INSERT INTO shared_space.people (user_id) VALUES ($1) RETURNING id")
            .bind(uid)
            .fetch_one(rt.pool())
            .await
            .unwrap();
    let project: Uuid = sqlx::query_scalar(
        "INSERT INTO shared_space.projects (name) VALUES ('shared') RETURNING id",
    )
    .fetch_one(rt.pool())
    .await
    .unwrap();
    sqlx::query("INSERT INTO shared_space.projects (name) VALUES ('hidden')")
        .execute(rt.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO shared_space.assignments (person_id,project_id) VALUES ($1,$2)")
        .bind(person)
        .bind(project)
        .execute(rt.pool())
        .await
        .unwrap();
    let read = invoke(
        &rt,
        &token,
        "query_data",
        json!({"app":"shared_space","entity":"projects"}),
    )
    .await;
    assert_eq!(read["result"].as_array().unwrap().len(), 1, "{read}");
    assert_eq!(read["result"][0]["id"], project.to_string(), "{read}");
    let metadata = invoke(&rt, &token, "describe_app", json!({"app":"shared_space"})).await;
    assert_eq!(
        metadata["result"]["dataContract"],
        json!([{"entityName":"projects","fields":[{"name":"name","type":"text"}]}]),
        "{metadata}"
    );
    let denied = invoke(&rt, &token, "mutate_data", json!({
        "app":"shared_space","entity":"projects","action":"update","id":project,"data":{"name":"forbidden"}
    })).await;
    assert!(
        denied["error"].is_string(),
        "shared read allowed write: {denied}"
    );
    sqlx::query("UPDATE shared_space.assignments SET ended_at=CURRENT_DATE")
        .execute(rt.pool())
        .await
        .unwrap();
    let revoked = invoke(
        &rt,
        &token,
        "query_data",
        json!({"app":"shared_space","entity":"projects"}),
    )
    .await;
    assert_eq!(
        revoked["result"],
        json!([]),
        "revoked assignment stayed readable: {revoked}"
    );
    rt.shutdown().await;
}
