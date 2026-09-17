//! Approved action contracts across public HTTP, real Bun workers and PostgreSQL.
//! The parent governance target registers this module.

use crate::harness::{self, TestRuntime};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::Arc, time::Duration};
use uuid::Uuid;

const APP: &str = "support_actions";
const APPROVALS: &str = "/api/v1/apps/support_actions/action-approvals";
const ACTIONS: [&str; 4] = [
    "assigned_projects",
    "add_interaction",
    "data_probe",
    "atomic_probe",
];
const TABLES: [&str; 7] = [
    "person",
    "project",
    "assignment",
    "interaction",
    "activity_log",
    "scratch",
    "billing",
];

const BACKEND: &[u8] = br#"
const workerId = crypto.randomUUID();
const build = "original";
const rows = result => result.rows.map(row =>
  Object.fromEntries(result.columns.map((column, i) => [column, row[i]])));
const capture = async run => {
  try { return { value: await run() }; }
  catch (error) { return { error: error.message }; }
};
const assigned = async (db, project, user) => {
  const result = await db.sql(
    `SELECT id FROM support_actions.assignment
     WHERE project_id = $1 AND user_id = $2 AND revoked_at IS NULL FOR SHARE`,
    [project, user]);
  if (!result.rows.length) throw new Error("project is not assigned to caller");
};
const add_interaction = async (params, caller, ctx) => {
  if (typeof params.message !== "string" || !params.message.trim())
    throw new Error("message is required");
  return ctx.transaction(async tx => {
    await assigned(tx, params.projectId, caller.userId);
    const result = await tx.sql(
      `INSERT INTO support_actions.interaction (project_id, author_id, message)
       VALUES ($1, $2, $3) RETURNING id::text, project_id::text, author_id::text, message`,
      [params.projectId, caller.userId, params.message]);
    await tx.sql(
      `INSERT INTO support_actions.activity_log (interaction_id, author_id, message)
       VALUES ($1, $2, 'interaction recorded')`,
      [result.rows[0][0], caller.userId]);
    return rows(result)[0];
  });
};

// There is no public ctx tool API. This adversarial probe exercises the actual
// IPC capability, rather than passing because an invented JS method is absent.
const toolCalls = new Map();
let input = "";
process.stdin.on("data", chunk => {
  input += chunk;
  let newline;
  while ((newline = input.indexOf("\n")) >= 0) {
    const line = input.slice(0, newline); input = input.slice(newline + 1);
    const message = JSON.parse(line);
    if (message.type === "agent_tool_result") toolCalls.get(message.call_id)?.(message);
  }
});
const toolProbe = () => new Promise((resolve, reject) => {
  const callId = crypto.randomUUID();
  const timer = setTimeout(() => {
    toolCalls.delete(callId); reject(new Error("tool probe timed out"));
  }, 5000);
  toolCalls.set(callId, message => {
    clearTimeout(timer); toolCalls.delete(callId);
    if (message.error) reject(new Error(message.error)); else resolve(message.result);
  });
  process.stdout.write(JSON.stringify({
    type: "agent_tool_call", invoke_id: callId, call_id: callId,
    tool_name: "query_data", args: { entity: "project" }
  }) + "\n");
});

const probe = async (params, caller, ctx) => {
  switch (params.operation) {
    case "identity":
      return { workerId, build, projects: rows(await ctx.sql(
        "SELECT id::text, internal_notes FROM support_actions.project ORDER BY id")) };
    case "sql":
      return params.transaction
        ? ctx.transaction(tx => tx.sql(params.sql, params.args ?? []))
        : ctx.sql(params.sql, params.args ?? []);
    case "collection":
      return ctx.collection(params.entity)[params.verb](...(params.args ?? []));
    case "credentials":
      return { context: ctx.credentials?.SUPPORT_TOKEN ?? null,
        environment: process.env.SUPPORT_TOKEN ?? null };
    case "job": return ctx.enqueueJob({ marker: "approved-action-job" });
    case "tool": return toolProbe();
    case "subcall": return ctx.action("add_interaction", params.input);
    case "selfAction": return ctx.selfAction("triggerAction", {
      actionName: "add_interaction", input: params.input });
    case "crossApp": return ctx.remote("support_archive").collection("records").find();
    case "held":
      // The loopback gate holds no database transaction/approval lock across
      // revocation and does not depend on worker filesystem permissions.
      await fetch(params.gate + "/" + workerId);
      return ctx.sql(
        "INSERT INTO support_actions.scratch (label) VALUES ('stale worker wrote') RETURNING id");
    default: throw new Error("unknown probe operation");
  }
};
serve({
  rpc: {
    assigned_projects: async (_params, caller, ctx) => {
      const projects = rows(await ctx.sql(
        `SELECT p.id::text, p.title, person.display_name AS person_name
         FROM support_actions.project p
         JOIN support_actions.person person ON person.id = p.person_id
         WHERE EXISTS (SELECT 1 FROM support_actions.assignment a
           WHERE a.project_id = p.id AND a.user_id = $1 AND a.revoked_at IS NULL)
         ORDER BY p.title`, [caller.userId]));
      const interactions = rows(await ctx.sql(
        `SELECT i.id::text, i.project_id::text, i.author_id::text, i.message
         FROM support_actions.interaction i
         WHERE EXISTS (SELECT 1 FROM support_actions.assignment a
           WHERE a.project_id = i.project_id AND a.user_id = $1 AND a.revoked_at IS NULL)
         ORDER BY i.message`, [caller.userId]));
      return { projects, interactions };
    },
    add_interaction,
    data_probe: (params, caller, ctx) => capture(() => probe(params, caller, ctx)),
    normal_probe: (params, caller, ctx) => capture(() => probe(params, caller, ctx)),
    atomic_probe: (params, caller, ctx) => capture(() => ctx.transaction(async tx => {
      await assigned(tx, params.projectId, caller.userId);
      const result = await tx.sql(
        `INSERT INTO support_actions.interaction (project_id, author_id, message)
         VALUES ($1, $2, $3) RETURNING id::text`,
        [params.projectId, caller.userId, params.message]);
      if (params.gate) {
        const identity = await tx.sql("SELECT pg_backend_pid()");
        await fetch(params.gate + "/" + identity.rows[0][0]);
      }
      if (params.fault === "callback") throw new Error("follow-up validation failed");
      if (params.fault === "caughtSql") {
        // A caught policy error must still poison the callback transaction.
        try { await tx.sql("INSERT INTO support_actions.project (title) VALUES ('forbidden project')"); }
        catch {}
        return { swallowed: true };
      }
      await tx.sql(
        `INSERT INTO support_actions.activity_log (interaction_id, author_id, message)
         VALUES ($1, $2, 'interaction recorded')`, [result.rows[0][0], caller.userId]);
      return { id: result.rows[0][0] };
    })),
  },
  onJob: async (_payload, _caller, ctx) => {
    await ctx.sql("INSERT INTO support_actions.scratch (label) VALUES ('job ran')");
  },
});
"#;

fn authority(action: &str) -> Value {
    match action {
        "assigned_projects" => json!({"data": {
            "assignment": ["read"], "project": ["read"],
            "person": ["read"], "interaction": ["read"]
        }}),
        // PostgreSQL applies UPDATE USING as well as SELECT policies to FOR SHARE.
        "add_interaction" | "atomic_probe" => json!({"data": {
            "assignment": ["read", "update"], "interaction": ["read", "create"],
            "activity_log": ["create"]
        }}),
        "data_probe" => json!({"data": {
            "assignment": ["read"], "project": ["read"], "person": ["read"],
            "interaction": ["read", "create"], "activity_log": ["create"],
            "scratch": ["read", "create", "update", "delete"]
        }}),
        _ => unreachable!(),
    }
}

fn manifest() -> Value {
    let link = |name: &str, entity: &str| {
        json!({
            "name": name, "type": "entity_link", "references": {"entity": entity, "field": "id"}
        })
    };
    json!({
        "appId": APP, "name": "Support desk", "version": "1.0.0",
        "actions": ACTIONS.map(|id| json!({
            "id": id, "name": id, "authority": authority(id)
        })),
        "dataContract": [
            {"entityName": "person", "fields": [
                {"name": "display_name", "type": "text"},
                {"name": "confidential", "type": "text", "sensitive": true}
            ]},
            {"entityName": "project", "fields": [
                link("person_id", "person"), {"name": "title", "type": "text"},
                {"name": "internal_notes", "type": "text"}
            ]},
            {"entityName": "assignment", "fields": [
                link("project_id", "project"), link("user_id", "core:users"),
                {"name": "revoked_at", "type": "date"}
            ]},
            {"entityName": "interaction", "fields": [
                link("project_id", "project"), link("author_id", "core:users"),
                {"name": "message", "type": "text", "required": true}
            ]},
            {"entityName": "activity_log", "fields": [
                link("interaction_id", "interaction"), link("author_id", "core:users"),
                {"name": "message", "type": "text"}
            ]},
            {"entityName": "scratch", "fields": [{"name": "label", "type": "text"}]},
            {"entityName": "billing", "fields": [{"name": "label", "type": "text"}]}
        ]
    })
}

struct Actor {
    token: String,
    id: Uuid,
}

struct Fixture {
    rt: TestRuntime,
    actors: Vec<Actor>,
    projects: Vec<Value>,
    interactions: Vec<Value>,
    assignments: Vec<Value>,
}

struct WorkerGate {
    url: String,
    arrivals: tokio::sync::mpsc::Receiver<String>,
    release: Arc<tokio::sync::Notify>,
    server: tokio::task::JoinHandle<()>,
}

impl WorkerGate {
    async fn start() -> Self {
        let (entered, arrivals) = tokio::sync::mpsc::channel::<String>(1);
        let release = Arc::new(tokio::sync::Notify::new());
        let signal = Arc::clone(&release);
        let router = axum::Router::new().route(
            "/gate/{marker}",
            axum::routing::get(
                move |axum::extract::Path(marker): axum::extract::Path<String>| {
                    let entered = entered.clone();
                    let release = Arc::clone(&signal);
                    async move {
                        entered.send(marker).await.unwrap();
                        release.notified().await;
                        "released"
                    }
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/gate", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Self {
            url,
            arrivals,
            release,
            server,
        }
    }
}

impl Drop for WorkerGate {
    fn drop(&mut self) {
        // Failed assertions must not strand a callback behind the test gate.
        self.release.notify_one();
        self.server.abort();
    }
}

fn key(action: &str) -> String {
    format!("app:{APP}:action:{action}")
}

async fn actor(rt: &TestRuntime, name: &str) -> Actor {
    let token = rt.register_and_login(&format!("{name}@support.test")).await;
    let (status, me) = rt
        .request_as(Method::GET, "/api/v1/auth/me", &token, None)
        .await;
    assert_eq!(status, StatusCode::OK, "fixture identity: {me}");
    let id = Uuid::parse_str(me["id"].as_str().unwrap()).unwrap();
    let (status, assignments) = rt.get_json("/api/v1/roles/assignments").await;
    assert_eq!(status, StatusCode::OK, "fixture roles: {assignments}");
    for assignment in assignments
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["userId"] == id.to_string())
    {
        let (status, body) = rt
            .post_json(
                "/api/v1/roles/revoke",
                &json!({
                    "userId": id, "role": assignment["role"]
                }),
            )
            .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "remove fixture default role: {body}"
        );
    }
    let (status, body) = rt
        .post_json(
            "/api/v1/roles/assign",
            &json!({
                "userId": id, "role": "support_operator"
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "assign fixture role: {body}");
    Actor { token, id }
}

async fn fixture() -> Fixture {
    let rt = TestRuntime::boot().await;
    rt.install_manifest(&manifest()).await;
    let (status, body) = rt
        .post_json(
            "/api/v1/roles",
            &json!({
                "name": "support_operator", "permissions": ACTIONS.map(key)
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "fixture role: {body}");
    let actors = vec![actor(&rt, "alex").await, actor(&rt, "blair").await];
    let mut projects = vec![];
    let mut interactions = vec![];
    for (title, display_name) in [("Alpha", "Ada"), ("Beta", "Bea"), ("Private", "Cy")] {
        let person = rt
            .create(
                APP,
                "person",
                &json!({
                    "display_name": display_name, "confidential": format!("sensitive-{title}")
                }),
            )
            .await;
        let project = rt.create(APP, "project", &json!({
            "title": title, "person_id": person["id"], "internal_notes": format!("internal-{title}")
        })).await;
        let interaction = rt
            .create(
                APP,
                "interaction",
                &json!({
                    "project_id": project["id"], "author_id": actors[1].id,
                    "message": format!("{title} initial contact")
                }),
            )
            .await;
        rt.create(
            APP,
            "activity_log",
            &json!({
                "interaction_id": interaction["id"], "author_id": actors[1].id,
                "message": "initial contact recorded"
            }),
        )
        .await;
        interactions.push(interaction);
        projects.push(project);
    }
    let mut assignments = vec![];
    for (actor_index, project_index) in [(0, 0), (1, 0), (1, 1)] {
        assignments.push(
            rt.create(
                APP,
                "assignment",
                &json!({
                    "project_id": projects[project_index]["id"], "user_id": actors[actor_index].id
                }),
            )
            .await,
        );
    }
    rt.create(APP, "scratch", &json!({"label": "existing scratch"}))
        .await;
    rt.create(APP, "billing", &json!({"label": "billing canary"}))
        .await;
    let (status, body) = rt
        .deploy(APP, &harness::make_tar_gz(&[("index.ts", BACKEND)]))
        .await;
    assert_eq!(status, StatusCode::OK, "fixture deploy: {body}");
    Fixture {
        rt,
        actors,
        projects,
        interactions,
        assignments,
    }
}

async fn call(rt: &TestRuntime, token: &str, action: &str, params: Value) -> (StatusCode, Value) {
    rt.request_as(
        Method::POST,
        &format!("/api/v1/apps/{APP}/rpc"),
        token,
        Some(&json!({"method": action, "params": params})),
    )
    .await
}

fn binding(review: &Value) -> Value {
    json!({
        "revision": review["revision"], "backendDigest": review["backendDigest"],
        "installationId": review["installationId"]
    })
}

fn entry<'a>(review: &'a Value, action: &str) -> &'a Value {
    review["actions"]
        .as_array()
        .expect("approval actions")
        .iter()
        .find(|row| row["id"] == action)
        .expect("declared action in review")
}

async fn approve(rt: &TestRuntime, actions: &[&str]) -> Value {
    let (status, review) = rt.get_json(APPROVALS).await;
    assert_eq!(status, StatusCode::OK, "fixture approval review: {review}");
    for action in actions {
        let (status, body) = rt
            .post_json(&format!("{APPROVALS}/{action}"), &binding(&review))
            .await;
        assert!(
            status.is_success(),
            "fixture approve {action}: {status} {body}"
        );
    }
    review
}

async fn snapshot(rt: &TestRuntime) -> Value {
    let mut result = serde_json::Map::new();
    for table in TABLES {
        let rows: Value = sqlx::query_scalar(&format!(
            "SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY id), '[]'::jsonb) FROM {APP}.{table} t"
        ))
        .fetch_one(rt.pool())
        .await
        .unwrap();
        result.insert(table.into(), rows);
    }
    Value::Object(result)
}

async fn referential_rows(rt: &TestRuntime) -> Value {
    sqlx::query_scalar(
        "SELECT jsonb_build_object(
           'parents', (SELECT coalesce(jsonb_agg(to_jsonb(p) ORDER BY p.id), '[]'::jsonb)
                       FROM (SELECT id::text, label FROM support_actions.parent) p),
           'children', (SELECT coalesce(jsonb_agg(to_jsonb(c) ORDER BY c.id), '[]'::jsonb)
                        FROM (SELECT id::text, parent_id::text, label FROM support_actions.child) c))",
    ).fetch_one(rt.pool()).await.unwrap()
}

fn project_display(project: &Value, person_name: &str) -> Value {
    json!({"id": project["id"], "title": project["title"], "person_name": person_name})
}

fn interaction_display(interaction: &Value) -> Value {
    json!({
        "id": interaction["id"], "project_id": interaction["project_id"],
        "author_id": interaction["author_id"], "message": interaction["message"]
    })
}

#[tokio::test]
async fn review_binds_approval_to_a_deployment_and_only_admin_can_change_it() {
    let rt = TestRuntime::boot().await;
    rt.install_manifest(&manifest()).await;
    let (status, empty) = rt.get_json(APPROVALS).await;
    assert_eq!(status, StatusCode::OK, "{empty}");
    assert_eq!(empty["revision"], Value::Null, "{empty}");
    assert_eq!(empty["backendDigest"], Value::Null, "{empty}");
    assert!(
        Uuid::parse_str(empty["installationId"].as_str().unwrap()).is_ok(),
        "{empty}"
    );
    for action in ACTIONS {
        assert_eq!(
            entry(&empty, action),
            &json!({
                "id": action, "authority": authority(action), "approvalId": null, "status": "pending"
            })
        );
    }
    let (status, body) = rt
        .post_json(&format!("{APPROVALS}/data_probe"), &binding(&empty))
        .await;
    assert!(
        matches!(
            status,
            StatusCode::BAD_REQUEST | StatusCode::CONFLICT | StatusCode::UNPROCESSABLE_ENTITY
        ),
        "no deployment is approvable: {status} {body}"
    );
    let (status, body) = rt
        .deploy(APP, &harness::make_tar_gz(&[("index.ts", BACKEND)]))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, review) = rt.get_json(APPROVALS).await;
    assert_eq!(status, StatusCode::OK, "{review}");
    assert_eq!(review["installationId"], empty["installationId"]);
    assert!(
        Uuid::parse_str(review["revision"].as_str().unwrap()).is_ok(),
        "{review}"
    );
    assert!(
        !review["backendDigest"].as_str().unwrap().is_empty(),
        "{review}"
    );
    assert_eq!(
        review["actions"].as_array().unwrap().len(),
        ACTIONS.len(),
        "{review}"
    );
    let (status, body) = rt
        .post_json(
            "/api/v1/roles",
            &json!({
                "name": "support_operator", "permissions": [key("data_probe")]
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let nonadmin = actor(&rt, "reviewer").await;
    let path = format!("{APPROVALS}/data_probe");
    for method in [Method::POST, Method::DELETE] {
        let (status, body) = rt
            .request_as(
                method.clone(),
                &path,
                &nonadmin.token,
                Some(&binding(&review)),
            )
            .await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{method} requires admin: {body}"
        );
    }
    for (field, bad_value) in [
        ("revision", json!(Uuid::new_v4())),
        ("backendDigest", json!("wrong-deployed-bytes")),
        ("installationId", json!(Uuid::new_v4())),
    ] {
        let mut stale = binding(&review);
        stale[field] = bad_value;
        let (status, body) = rt.post_json(&path, &stale).await;
        assert!(
            matches!(status, StatusCode::BAD_REQUEST | StatusCode::CONFLICT),
            "{field} mismatch must not approve: {status} {body}"
        );
        let (status, after) = rt.get_json(APPROVALS).await;
        assert_eq!(status, StatusCode::OK, "{after}");
        assert_eq!(
            entry(&after, "data_probe")["status"],
            "pending",
            "{field}: {after}"
        );
        assert_eq!(
            entry(&after, "data_probe")["approvalId"],
            Value::Null,
            "{field}: {after}"
        );
    }
    let (status, body) = rt.post_json(&path, &binding(&review)).await;
    assert!(status.is_success(), "{status} {body}");
    let (status, approved) = rt.get_json(APPROVALS).await;
    assert_eq!(status, StatusCode::OK, "{approved}");
    assert_eq!(binding(&approved), binding(&review));
    for action in ACTIONS {
        assert_eq!(entry(&approved, action)["authority"], authority(action));
        if action == "data_probe" {
            assert_eq!(entry(&approved, action)["status"], "approved");
            assert!(
                Uuid::parse_str(entry(&approved, action)["approvalId"].as_str().unwrap()).is_ok()
            );
        } else {
            assert_eq!(entry(&approved, action)["status"], "pending", "{action}");
            assert_eq!(
                entry(&approved, action)["approvalId"],
                Value::Null,
                "{action}"
            );
        }
    }
    // A non-admin revoke must also fail when an approval actually exists.
    let (status, body) = rt
        .request_as(Method::DELETE, &path, &nonadmin.token, None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (_, after) = rt.get_json(APPROVALS).await;
    assert_eq!(entry(&after, "data_probe"), entry(&approved, "data_probe"));
    rt.shutdown().await;
}

#[tokio::test]
async fn approval_rejects_archives_without_a_regular_backend_entrypoint() {
    let f = fixture().await;
    let mut previous = approve(&f.rt, &["data_probe"]).await;
    for (scenario, path) in [
        ("readme only", "README.md"),
        ("directory named index.ts", "index.ts/README.md"),
    ] {
        let archive = harness::make_tar_gz(&[(path, b"Documentation only.")]);
        let (deploy_status, deploy_body) = f.rt.deploy(APP, &archive).await;
        let entrypoint =
            f.rt.runtime
                .data_dir()
                .join("apps")
                .join(APP)
                .join("index.ts");
        if scenario == "readme only" {
            assert!(!entrypoint.exists(), "{scenario}: unexpected entrypoint");
        } else {
            assert!(
                entrypoint.is_dir(),
                "{scenario}: fixture must contain an entrypoint directory"
            );
        }
        // Startup may fail after deployment has already recorded the release.
        // A missing digest must not make this pass at the stale-review gate.
        let (status, review) = f.rt.get_json(APPROVALS).await;
        assert_eq!(status, StatusCode::OK, "{scenario}: {review}");
        assert!(
            review["revision"]
                .as_str()
                .is_some_and(|id| Uuid::parse_str(id).is_ok()),
            "{scenario}: deployment did not record a revision: {deploy_status} {deploy_body}; {review}"
        );
        assert!(
            review["backendDigest"]
                .as_str()
                .is_some_and(|digest| !digest.is_empty()),
            "{scenario}: deployment did not record a digest: {deploy_status} {deploy_body}; {review}"
        );
        assert_ne!(review["revision"], previous["revision"], "{scenario}");
        let (status, body) =
            f.rt.post_json(&format!("{APPROVALS}/data_probe"), &binding(&review))
                .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{scenario}: {body}");
        assert!(
            body.to_string().to_lowercase().contains("entry"),
            "{scenario}: approval must reject the missing regular entrypoint: {body}"
        );
        let (status, after) = f.rt.get_json(APPROVALS).await;
        assert_eq!(status, StatusCode::OK, "{scenario}: {after}");
        assert_eq!(binding(&after), binding(&review), "{scenario}");
        assert_eq!(
            entry(&after, "data_probe")["status"],
            "pending",
            "{scenario}: {after}"
        );
        assert_eq!(
            entry(&after, "data_probe")["approvalId"],
            Value::Null,
            "{scenario}: {after}"
        );
        previous = after;
    }
    f.rt.shutdown().await;
}

#[tokio::test]
async fn support_actions_return_only_assigned_projects_with_safe_joined_display_data() {
    let f = fixture().await;
    approve(&f.rt, &["assigned_projects"]).await;
    for (index, actor) in f.actors.iter().enumerate() {
        let (status, permissions) =
            f.rt.request_as(Method::GET, "/api/v1/permissions", &actor.token, None)
                .await;
        assert_eq!(status, StatusCode::OK, "{permissions}");
        let actual: BTreeSet<_> = permissions["permissions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p.as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            actual,
            ACTIONS.map(key).into_iter().collect(),
            "actor {index} must have action grants only"
        );
        let (status, body) = call(
            &f.rt,
            &actor.token,
            "assigned_projects",
            json!({
                "userId": f.actors[1 - index].id, "projectId": f.projects[2]["id"],
                "authority": {"data": {"project": ["read"], "billing": ["read"]}}
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "actor {index}: {body}");
        let mut projects = vec![project_display(&f.projects[0], "Ada")];
        let mut interactions = vec![interaction_display(&f.interactions[0])];
        if index == 1 {
            projects.push(project_display(&f.projects[1], "Bea"));
            interactions.push(interaction_display(&f.interactions[1]));
        }
        assert_eq!(
            body,
            json!({"projects": projects, "interactions": interactions}),
            "actor {index}: exact projection must omit internal_notes and unrelated records"
        );
    }
    f.rt.shutdown().await;
}

#[tokio::test]
async fn recording_interactions_forces_the_author_and_rejects_forged_targets() {
    let f = fixture().await;
    approve(&f.rt, &["add_interaction", "assigned_projects"]).await;
    for (index, actor) in f.actors.iter().enumerate() {
        let message = format!("operator {index} follow-up");
        let (status, body) = call(
            &f.rt,
            &actor.token,
            "add_interaction",
            json!({
                "projectId": f.projects[0]["id"], "message": message,
                "author_id": f.actors[1 - index].id, "authorId": f.actors[1 - index].id,
                "userId": f.actors[1 - index].id
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "actor {index}: {body}");
        assert_eq!(
            body,
            json!({
                "id": body["id"], "project_id": f.projects[0]["id"],
                "author_id": actor.id, "message": message
            })
        );
        let saved = snapshot(&f.rt).await;
        let interaction = saved["interaction"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == body["id"])
            .unwrap();
        assert_eq!(
            interaction["author_id"],
            actor.id.to_string(),
            "{interaction}"
        );
        assert_eq!(
            interaction["project_id"], f.projects[0]["id"],
            "{interaction}"
        );
        let audit: Vec<_> = saved["activity_log"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["interaction_id"] == body["id"])
            .collect();
        assert_eq!(audit.len(), 1, "one audit per interaction: {saved}");
        assert_eq!(audit[0]["author_id"], actor.id.to_string());
        let (status, displayed) = call(
            &f.rt,
            &f.actors[1 - index].token,
            "assigned_projects",
            json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{displayed}");
        assert!(
            displayed["interactions"]
                .as_array()
                .unwrap()
                .contains(&body),
            "the other assigned reader sees the committed follow-up: {displayed}"
        );
    }
    let before = snapshot(&f.rt).await;
    for target in [
        f.projects[1]["id"].clone(),
        f.projects[2]["id"].clone(),
        json!(Uuid::new_v4()),
    ] {
        let (status, body) = call(
            &f.rt,
            &f.actors[0].token,
            "add_interaction",
            json!({
                "projectId": target, "message": "forged follow-up",
                "userId": f.actors[1].id, "author_id": f.actors[1].id
            }),
        )
        .await;
        assert!(
            !status.is_success(),
            "unassigned target {target}: {status} {body}"
        );
        assert!(
            body.to_string().contains("project is not assigned"),
            "{target}: {body}"
        );
        assert_eq!(
            snapshot(&f.rt).await,
            before,
            "forged target {target} persisted data"
        );
    }
    f.rt.shutdown().await;
}

#[tokio::test]
async fn revoking_one_assignment_leaves_the_other_reader_and_shared_data_intact() {
    let f = fixture().await;
    approve(&f.rt, &["assigned_projects", "add_interaction"]).await;
    let (status, before) = call(&f.rt, &f.actors[1].token, "assigned_projects", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{before}");
    let (status, warm) = call(&f.rt, &f.actors[0].token, "assigned_projects", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{warm}");
    assert_eq!(warm["projects"].as_array().unwrap().len(), 1);
    for mutation in ["end", "delete"] {
        let path = format!(
            "/api/v1/apps/{APP}/collections/assignment/{}",
            f.assignments[0]["id"].as_str().unwrap()
        );
        if mutation == "delete" {
            let (status, body) = f.rt.patch_json(&path, &json!({"revoked_at": null})).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            let (status, restored) =
                call(&f.rt, &f.actors[0].token, "assigned_projects", json!({})).await;
            assert_eq!(status, StatusCode::OK, "{restored}");
            assert_eq!(restored, warm, "deletion must revoke an active assignment");
        }
        let (status, body) = if mutation == "end" {
            f.rt.patch_json(&path, &json!({"revoked_at": "2099-01-01"}))
                .await
        } else {
            f.rt.delete_json(&path).await
        };
        assert!(status.is_success(), "{mutation}: {body}");
        let (status, revoked) =
            call(&f.rt, &f.actors[0].token, "assigned_projects", json!({})).await;
        assert_eq!(status, StatusCode::OK, "{mutation}: {revoked}");
        assert_eq!(
            revoked,
            json!({"projects": [], "interactions": []}),
            "{mutation}"
        );
        let (status, other) = call(&f.rt, &f.actors[1].token, "assigned_projects", json!({})).await;
        assert_eq!(status, StatusCode::OK, "{mutation}: {other}");
        assert_eq!(other, before, "{mutation}: shared-role reader lost access");
        let saved = snapshot(&f.rt).await;
        let (status, body) = call(
            &f.rt,
            &f.actors[0].token,
            "add_interaction",
            json!({
                "projectId": f.projects[0]["id"], "message": "after assignment revocation"
            }),
        )
        .await;
        assert!(!status.is_success(), "{mutation}: {body}");
        assert!(
            body.to_string().contains("project is not assigned"),
            "{mutation}: {body}"
        );
        assert_eq!(
            snapshot(&f.rt).await,
            saved,
            "{mutation}: revoked author persisted data"
        );
    }
    f.rt.shutdown().await;
}

#[tokio::test]
async fn caller_grants_are_per_action_and_never_confer_direct_collection_authority() {
    let f = fixture().await;
    approve(&f.rt, &ACTIONS).await;
    let before = snapshot(&f.rt).await;
    for actor in &f.actors {
        for table in TABLES {
            let base = format!("/api/v1/apps/{APP}/collections/{table}");
            let seed = before[table][0]
                .as_object()
                .expect("persisted target for every table");
            let id = seed["id"].as_str().unwrap();
            let payload: serde_json::Map<String, Value> = seed
                .iter()
                .filter(|(name, _)| !matches!(name.as_str(), "id" | "created_at" | "updated_at"))
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect();
            let (status, body) =
                f.rt.request_as(Method::GET, &base, &actor.token, None)
                    .await;
            if status == StatusCode::OK {
                assert_eq!(body, json!([]), "action grant leaked direct {table} rows");
            } else {
                assert_eq!(status, StatusCode::FORBIDDEN, "GET {base}: {body}");
            }
            let requests = [
                (
                    Method::POST,
                    base.clone(),
                    Some(Value::Object(payload.clone())),
                ),
                (Method::GET, format!("{base}/{id}"), None),
                (
                    Method::PATCH,
                    format!("{base}/{id}"),
                    Some(Value::Object(payload)),
                ),
                (Method::DELETE, format!("{base}/{id}"), None),
            ];
            for (method, path, payload) in requests {
                let (status, body) =
                    f.rt.request_as(method.clone(), &path, &actor.token, payload.as_ref())
                        .await;
                if method == Method::POST {
                    assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}: {body}");
                } else {
                    assert!(
                        matches!(status, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
                        "{method} {path}: {status} {body}"
                    );
                }
            }
        }
    }
    assert_eq!(snapshot(&f.rt).await, before);
    for (label, permissions, allowed) in [
        ("plain invoke", vec![format!("app:{APP}:invoke")], false),
        ("wrong action", vec![key("add_interaction")], false),
        (
            "exact action without invoke",
            vec![key("assigned_projects")],
            true,
        ),
        ("no grants", vec![], false),
    ] {
        let (status, body) =
            f.rt.patch_json(
                "/api/v1/roles/support_operator",
                &json!({"permissions": permissions}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{label}: {body}");
        let (status, body) = call(
            &f.rt,
            &f.actors[0].token,
            "assigned_projects",
            json!({
                "action": "assigned_projects", "actionId": "assigned_projects",
                "authority": authority("assigned_projects"), "effectivePerms": ["*"]
            }),
        )
        .await;
        if allowed {
            assert_eq!(status, StatusCode::OK, "{label}: {body}");
            assert_eq!(
                body["projects"],
                json!([project_display(&f.projects[0], "Ada")])
            );
        } else {
            assert_eq!(status, StatusCode::FORBIDDEN, "{label}: {body}");
        }
    }
    f.rt.shutdown().await;
}

#[tokio::test]
async fn unapproved_and_normal_calls_cannot_borrow_action_authority_from_parameters() {
    let f = fixture().await;
    let before = snapshot(&f.rt).await;
    for (label, token) in [("operator", &f.actors[0].token), ("admin", &f.rt.token)] {
        for action in ACTIONS {
            let (status, body) = call(&f.rt, token, action, json!({
                "operation": "identity", "projectId": f.projects[0]["id"], "message": "unapproved",
                "authority": authority(action)
            })).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{label}/{action}: {body}");
        }
    }
    let review = approve(&f.rt, &["data_probe"]).await;
    let (status, body) =
        f.rt.patch_json(
            "/api/v1/roles/support_operator",
            &json!({"permissions": [format!("app:{APP}:invoke"), key("data_probe")]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, approved) = f.rt.get_json(APPROVALS).await;
    assert_eq!(status, StatusCode::OK, "{approved}");
    for transaction in [false, true] {
        let (status, body) = call(
            &f.rt,
            &f.actors[0].token,
            "normal_probe",
            json!({
                "operation": "sql", "sql": "SELECT id FROM support_actions.project",
                "transaction": transaction, "action": "data_probe", "actionId": "data_probe",
                "authority": authority("data_probe"), "revision": review["revision"],
                "approvalId": entry(&approved, "data_probe")["approvalId"],
                "invocationKind": "action", "effectivePerms": ["*"]
            }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "ordinary RPC must reach Bun: {body}"
        );
        assert!(
            body["error"].is_string() || body["value"]["rows"] == json!([]),
            "normal tx={transaction} borrowed approved authority: {body}"
        );
    }
    let (status, body) = call(&f.rt, &f.actors[0].token, "normal_probe", json!({
        "operation": "sql", "sql": "INSERT INTO support_actions.scratch (label) VALUES ('forged')",
        "actionId": "data_probe", "authority": authority("data_probe")
    })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["error"].is_string(),
        "normal write borrowed authority: {body}"
    );
    assert_eq!(snapshot(&f.rt).await, before);
    f.rt.shutdown().await;
}

#[tokio::test]
async fn approved_sql_and_collections_obey_exact_local_tables_and_crud_verbs_even_for_admins() {
    let f = fixture().await;
    approve(&f.rt, &["data_probe"]).await;
    let mut project_ids: Vec<_> = f
        .projects
        .iter()
        .map(|project| project["id"].as_str().unwrap())
        .collect();
    project_ids.sort_unstable();
    let expected_locks: Vec<_> = project_ids.iter().map(|id| json!([id])).collect();
    for (label, token) in [("operator", &f.actors[0].token), ("admin", &f.rt.token)] {
        for transaction in [false, true] {
            let (status, locked) = call(
                &f.rt,
                token,
                "data_probe",
                json!({
                    "operation": "sql", "transaction": transaction,
                    "sql": "SELECT id::text FROM support_actions.project ORDER BY id FOR SHARE"
                }),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{label}/tx={transaction}: {locked}");
            assert_eq!(
                locked["value"]["rows"],
                json!(expected_locks),
                "{label}/tx={transaction}: read-only authority must lock every readable project"
            );
            for sql in [
                "INSERT INTO support_actions.scratch (label) VALUES ('ceiling') RETURNING label",
                "UPDATE support_actions.scratch SET label = 'updated' WHERE label = 'ceiling' RETURNING label",
                "SELECT label FROM support_actions.scratch WHERE label = 'updated'",
                "DELETE FROM support_actions.scratch WHERE label = 'updated' RETURNING label",
            ] {
                let (status, body) = call(
                    &f.rt,
                    token,
                    "data_probe",
                    json!({
                        "operation": "sql", "sql": sql, "transaction": transaction
                    }),
                )
                .await;
                assert_eq!(
                    status,
                    StatusCode::OK,
                    "{label}/tx={transaction}/{sql}: {body}"
                );
                assert_eq!(
                    body["value"]["rows"],
                    json!([[if sql.starts_with("INSERT") {
                        "ceiling"
                    } else {
                        "updated"
                    }]]),
                    "{label}/tx={transaction}/{sql}: {body}"
                );
            }
        }
        let (status, created) = call(&f.rt, token, "data_probe", json!({
            "operation": "collection", "entity": "scratch", "verb": "create", "args": [{"label": "collection"}]
        })).await;
        assert_eq!(status, StatusCode::OK, "{label}: {created}");
        assert_eq!(
            created["value"]["label"], "collection",
            "{label}: {created}"
        );
        let id = created["value"]["id"].clone();
        for (verb, args, expected) in [
            (
                "update",
                json!([id, {"label": "changed"}]),
                json!("changed"),
            ),
            ("findOne", json!([{"id": id}]), json!("changed")),
        ] {
            let (status, body) = call(
                &f.rt,
                token,
                "data_probe",
                json!({
                    "operation": "collection", "entity": "scratch", "verb": verb, "args": args
                }),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{label}/{verb}: {body}");
            assert_eq!(body["value"]["label"], expected, "{label}/{verb}: {body}");
        }
        let (status, body) = call(
            &f.rt,
            token,
            "data_probe",
            json!({
                "operation": "collection", "entity": "scratch", "verb": "delete", "args": [id]
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{label}: {body}");
        assert!(body.get("error").is_none(), "{label}: {body}");
        let saved = snapshot(&f.rt).await;
        assert!(
            !saved["scratch"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["id"] == id),
            "collection delete did not persist: {saved}"
        );
        let before = snapshot(&f.rt).await;
        for (table, verbs) in [
            ("billing", vec!["read", "create", "update", "delete"]),
            ("project", vec!["create", "update", "delete"]),
            ("interaction", vec!["update", "delete"]),
            ("activity_log", vec!["read", "update", "delete"]),
        ] {
            for verb in verbs {
                let field = if table == "project" {
                    "title"
                } else if table == "billing" {
                    "label"
                } else {
                    "message"
                };
                let sql = match verb {
                    "read" => format!("SELECT id FROM {APP}.{table}"),
                    "create" => {
                        format!("INSERT INTO {APP}.{table} ({field}) VALUES ('ceiling escaped')")
                    }
                    "update" => format!("UPDATE {APP}.{table} SET {field} = 'ceiling escaped'"),
                    "delete" => format!("DELETE FROM {APP}.{table}"),
                    _ => unreachable!(),
                };
                let mut statements = vec![sql];
                if table == "project" && verb == "update" {
                    // The technical UPDATE(id) grant permits row locks, not a
                    // write, even when the primary-key value would not change.
                    statements.push("UPDATE support_actions.project SET id = id".into());
                }
                for sql in statements {
                    for transaction in [false, true] {
                        let (status, body) = call(
                            &f.rt,
                            token,
                            "data_probe",
                            json!({
                                "operation": "sql", "sql": sql, "transaction": transaction,
                                "authority": {"data": {table: ["read", "create", "update", "delete"]}}
                            }),
                        )
                        .await;
                        assert_eq!(
                            status,
                            StatusCode::OK,
                            "{label}/{table}/{verb}/tx={transaction}/{sql}: {body}"
                        );
                        assert!(
                            body["error"].is_string(),
                            "out-of-ceiling SQL must be rejected, not silently filtered: {label}/{table}/{verb}/tx={transaction}/{sql}: {body}"
                        );
                    }
                }
                let id = before[table][0]["id"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| Uuid::new_v4().to_string());
                let (method, args) = match verb {
                    "read" => ("find", json!([])),
                    "create" => ("create", json!([{field: "ceiling escaped"}])),
                    "update" => ("update", json!([id, {field: "ceiling escaped"}])),
                    "delete" => ("delete", json!([id])),
                    _ => unreachable!(),
                };
                let (status, body) = call(
                    &f.rt,
                    token,
                    "data_probe",
                    json!({
                        "operation": "collection", "entity": table, "verb": method, "args": args
                    }),
                )
                .await;
                assert_eq!(status, StatusCode::OK, "{label}/{table}/{method}: {body}");
                assert!(
                    body["error"].is_string(),
                    "{label}/{table}/{method} escaped ceiling: {body}"
                );
                assert_eq!(
                    snapshot(&f.rt).await,
                    before,
                    "{label}/{table}/{verb} changed data"
                );
            }
        }
    }
    f.rt.shutdown().await;
}

#[tokio::test]
async fn ownership_resolver_cannot_leak_person_ids_outside_approved_data_authority() {
    let f = fixture().await;
    let mut owned = manifest();
    owned["dataContract"][0]["fields"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "name": "core_user_id", "type": "entity_link", "owner": true,
            "references": {"entity": "core:users", "field": "id"}
        }));
    owned["dataContract"][1]["fields"][0]["owner"] = json!(true);
    owned["actions"][2]["authority"] = json!({"data": {"scratch": ["read"]}});
    f.rt.install_manifest(&owned).await;
    let (status, body) =
        f.rt.patch_json(
            "/api/v1/roles/support_operator",
            &json!({
                "permissions": [format!("app:{APP}:invoke"), key("data_probe")]
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let resolver =
        r#"SELECT rootcx_system."rootcx_own.support_actions.person"()::text AS id ORDER BY id"#;
    for (index, actor) in f.actors.iter().enumerate() {
        let person_id = f.projects[index]["person_id"].as_str().unwrap();
        let (status, body) =
            f.rt.patch_json(
                &format!("/api/v1/apps/{APP}/collections/person/{person_id}"),
                &json!({"core_user_id": actor.id}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "actor {index}: {body}");
        for transaction in [false, true] {
            let (status, ordinary) = call(
                &f.rt,
                &actor.token,
                "normal_probe",
                json!({
                    "operation": "sql", "sql": resolver, "transaction": transaction
                }),
            )
            .await;
            assert_eq!(
                status,
                StatusCode::OK,
                "actor {index}/tx={transaction}: {ordinary}"
            );
            assert_eq!(
                ordinary["value"]["rows"],
                json!([[person_id]]),
                "actor {index}/tx={transaction}: real resolver must have an owned ID available to leak"
            );
        }
    }

    let before = snapshot(&f.rt).await;
    for person_read in [false, true] {
        if person_read {
            owned["actions"][2]["authority"]["data"]["person"] = json!(["read"]);
            f.rt.install_manifest(&owned).await;
        }
        let review = approve(&f.rt, &["data_probe"]).await;
        assert_eq!(
            entry(&review, "data_probe")["authority"],
            owned["actions"][2]["authority"]
        );
        for (index, actor) in f.actors.iter().enumerate() {
            for transaction in [false, true] {
                let (status, scratch) = call(
                    &f.rt,
                    &actor.token,
                    "data_probe",
                    json!({
                        "operation": "sql", "sql": "SELECT label FROM support_actions.scratch",
                        "transaction": transaction
                    }),
                )
                .await;
                assert_eq!(
                    status,
                    StatusCode::OK,
                    "person_read={person_read}/actor {index}/tx={transaction}: {scratch}"
                );
                assert_eq!(
                    scratch["value"]["rows"],
                    json!([["existing scratch"]]),
                    "person_read={person_read}/actor {index}/tx={transaction}: approved scratch read must work"
                );
                let (status, result) = call(
                    &f.rt,
                    &actor.token,
                    "data_probe",
                    json!({
                        "operation": "sql", "sql": resolver, "transaction": transaction
                    }),
                )
                .await;
                assert_eq!(
                    status,
                    StatusCode::OK,
                    "person_read={person_read}/actor {index}/tx={transaction}: {result}"
                );
                let expected = if person_read {
                    json!([[f.projects[index]["person_id"]]])
                } else {
                    json!([])
                };
                assert_eq!(
                    result["value"]["rows"], expected,
                    "person_read={person_read}/actor {index}/tx={transaction}: resolver must enforce the approval's person.read ceiling: {result}"
                );
            }
        }
    }
    assert_eq!(
        snapshot(&f.rt).await,
        before,
        "resolver probes must not change business data"
    );
    f.rt.shutdown().await;
}

#[tokio::test]
async fn identical_manifest_reinstall_preserves_approved_shared_resolver_execution() {
    let f = fixture().await;
    let mut shared = manifest();
    shared["dataContract"][0]["fields"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "name": "core_user_id", "type": "entity_link", "owner": true,
            "references": {"entity": "core:users", "field": "id"}
        }));
    shared["dataContract"][2]["fields"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "name": "member_id", "type": "entity_link",
            "references": {"entity": "person", "field": "id"}
        }));
    shared["dataContract"][2]["share"] = json!({
        "scope": "resource", "grantee": "member_id", "subject": "project_id",
        "activeWhen": {"isNull": "revoked_at"},
        "targets": [{"entity": "interaction", "via": ["project_id"]}]
    });
    f.rt.install_manifest(&shared).await;
    approve(&f.rt, &["data_probe"]).await;
    let (status, approved) = f.rt.get_json(APPROVALS).await;
    assert_eq!(status, StatusCode::OK, "{approved}");
    assert_eq!(entry(&approved, "data_probe")["status"], "approved");
    assert!(
        entry(&approved, "data_probe")["approvalId"].is_string(),
        "{approved}"
    );

    for phase in ["before reinstall", "after identical reinstall"] {
        if phase == "after identical reinstall" {
            f.rt.install_manifest(&shared).await;
            let (status, after) = f.rt.get_json(APPROVALS).await;
            assert_eq!(status, StatusCode::OK, "{after}");
            assert_eq!(binding(&after), binding(&approved), "{phase}");
            assert_eq!(
                entry(&after, "data_probe"),
                entry(&approved, "data_probe"),
                "identical reconciliation must preserve the existing approval without reapproval"
            );
        }
        for (entity, records) in [("project", &f.projects), ("interaction", &f.interactions)] {
            let mut ids: Vec<_> = records
                .iter()
                .map(|row| row["id"].as_str().unwrap())
                .collect();
            ids.sort_unstable();
            let expected: Vec<_> = ids.iter().map(|id| json!([id])).collect();
            for transaction in [false, true] {
                let (status, rows) = call(
                    &f.rt,
                    &f.actors[0].token,
                    "data_probe",
                    json!({
                        "operation": "sql", "transaction": transaction,
                        "sql": format!("SELECT id::text FROM {APP}.{entity} ORDER BY id")
                    }),
                )
                .await;
                assert_eq!(
                    status,
                    StatusCode::OK,
                    "{phase}/{entity}/tx={transaction}: {rows}"
                );
                assert_eq!(
                    rows["value"]["rows"],
                    json!(expected),
                    "{phase}/{entity}/tx={transaction}: approved reads must survive reconciliation"
                );
                // No member_id is populated. Calling the resolver explicitly
                // proves EXECUTE survived even if full-read RLS short-circuits.
                let (status, resolver) = call(&f.rt, &f.actors[0].token, "data_probe", json!({
                    "operation": "sql", "transaction": transaction,
                    "sql": format!("SELECT rootcx_system.\"rootcx_shared.{APP}.{entity}\"()::text AS id")
                })).await;
                assert_eq!(
                    status,
                    StatusCode::OK,
                    "{phase}/{entity}/tx={transaction}: {resolver}"
                );
                assert_eq!(
                    resolver["value"]["rows"],
                    json!([]),
                    "{phase}/{entity}/tx={transaction}: approved role must retain shared resolver EXECUTE: {resolver}"
                );
            }
        }
    }
    f.rt.shutdown().await;
}

#[tokio::test]
async fn parent_delete_requires_explicit_authority_for_catalog_foreign_key_effects() {
    let rt = TestRuntime::boot().await;
    let mut definition = json!({
        "appId": APP, "name": "Support retention", "version": "1.0.0",
        "actions": [{
            "id": "data_probe", "name": "Delete parent",
            "authority": {"data": {"parent": ["read", "delete"]}}
        }],
        "dataContract": [
            {"entityName": "parent", "fields": [{"name": "label", "type": "text"}]},
            {"entityName": "child", "fields": [
                {"name": "parent_id", "type": "uuid"}, {"name": "label", "type": "text"}
            ]}
        ]
    });
    rt.install_manifest(&definition).await;
    let (status, body) = rt
        .post_json(
            "/api/v1/roles",
            &json!({
                "name": "support_operator", "permissions": [key("data_probe")]
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let operator = actor(&rt, "retention").await;
    let backend = br#"
serve({ rpc: {
  data_probe: (params, _caller, ctx) => ctx.sql(
    "DELETE FROM support_actions.parent WHERE id = $1 RETURNING id::text", [params.id]),
} });
"#;
    let (status, body) = rt
        .deploy(APP, &harness::make_tar_gz(&[("index.ts", backend)]))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    for (scenario, sql_action, catalog_action, required, wrong) in [
        ("cascade", "CASCADE", "c", "delete", "update"),
        ("set_null", "SET NULL", "n", "update", "delete"),
    ] {
        definition["actions"][0]["authority"]["data"] = json!({"parent": ["read", "delete"]});
        rt.install_manifest(&definition).await;
        // A catalog-only constraint catches implementations that inspect just
        // entity_link/on_delete manifest fields and miss real database effects.
        sqlx::query(
            "ALTER TABLE support_actions.child DROP CONSTRAINT IF EXISTS catalog_child_parent",
        )
        .execute(rt.pool())
        .await
        .unwrap();
        sqlx::query(&format!(
            "ALTER TABLE support_actions.child ADD CONSTRAINT catalog_child_parent
             FOREIGN KEY (parent_id) REFERENCES support_actions.parent(id) ON DELETE {sql_action}"
        ))
        .execute(rt.pool())
        .await
        .unwrap();
        let target = rt
            .create(
                APP,
                "parent",
                &json!({"label": format!("{scenario} target")}),
            )
            .await;
        let unrelated = rt
            .create(
                APP,
                "parent",
                &json!({"label": format!("{scenario} unrelated")}),
            )
            .await;
        for (label, parent) in [
            ("first", target["id"].clone()),
            ("second", target["id"].clone()),
            ("unrelated", unrelated["id"].clone()),
            ("unlinked", Value::Null),
        ] {
            rt.create(
                APP,
                "child",
                &json!({
                    "parent_id": parent, "label": format!("{scenario} {label}")
                }),
            )
            .await;
        }
        let before = referential_rows(&rt).await;
        for (phase, child_operations) in [
            ("missing child authority", None),
            ("read only", Some(json!(["read"]))),
            ("wrong child mutation", Some(json!([wrong]))),
            ("matching child mutation", Some(json!([required]))),
        ] {
            let mut data = json!({"parent": ["read", "delete"]});
            if let Some(operations) = child_operations {
                data["child"] = operations;
            }
            definition["actions"][0]["authority"]["data"] = data;
            rt.install_manifest(&definition).await;
            let actual: String = sqlx::query_scalar(
                "SELECT confdeltype::text FROM pg_constraint
                 WHERE conrelid = 'support_actions.child'::regclass AND conname = 'catalog_child_parent'",
            ).fetch_one(rt.pool()).await.unwrap();
            assert_eq!(
                actual, catalog_action,
                "{scenario}/{phase}: fixture FK changed during install"
            );

            let (status, review) = rt.get_json(APPROVALS).await;
            assert_eq!(status, StatusCode::OK, "{scenario}/{phase}: {review}");
            let (status, response) = rt
                .post_json(&format!("{APPROVALS}/data_probe"), &binding(&review))
                .await;
            if phase == "matching child mutation" {
                assert!(
                    status.is_success(),
                    "{scenario}/{phase}: {status} {response}"
                );
            } else {
                assert_eq!(
                    status,
                    StatusCode::BAD_REQUEST,
                    "{scenario}/{phase}: {response}"
                );
                assert!(
                    response.to_string().contains(&format!("child.{required}")),
                    "{scenario}/{phase}: rejection must identify the required child operation: {response}"
                );
            }
            let (status, inspected) = rt.get_json(APPROVALS).await;
            assert_eq!(status, StatusCode::OK, "{scenario}/{phase}: {inspected}");
            let action = entry(&inspected, "data_probe");
            assert_eq!(
                action["authority"], definition["actions"][0]["authority"],
                "{scenario}/{phase}: approval must not add implicit child permissions"
            );
            if phase == "matching child mutation" {
                assert_eq!(action["status"], "approved", "{scenario}: {action}");
                assert!(
                    Uuid::parse_str(action["approvalId"].as_str().unwrap()).is_ok(),
                    "{action}"
                );
            } else {
                assert_eq!(action["status"], "pending", "{scenario}/{phase}: {action}");
                assert_eq!(
                    action["approvalId"],
                    Value::Null,
                    "{scenario}/{phase}: {action}"
                );
                let (status, body) = call(
                    &rt,
                    &operator.token,
                    "data_probe",
                    json!({"id": target["id"]}),
                )
                .await;
                assert_eq!(status, StatusCode::FORBIDDEN, "{scenario}/{phase}: {body}");
            }
            assert_eq!(
                referential_rows(&rt).await,
                before,
                "{scenario}/{phase}: approval or refused invocation changed business rows"
            );
        }

        let (status, deleted) = call(
            &rt,
            &operator.token,
            "data_probe",
            json!({"id": target["id"]}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{scenario}: {deleted}");
        assert_eq!(
            deleted["rows"],
            json!([[target["id"]]]),
            "{scenario}: {deleted}"
        );
        let mut expected = before;
        expected["parents"]
            .as_array_mut()
            .unwrap()
            .retain(|row| row["id"] != target["id"]);
        let children = expected["children"].as_array_mut().unwrap();
        if scenario == "cascade" {
            children.retain(|row| row["parent_id"] != target["id"]);
        } else {
            for child in children
                .iter_mut()
                .filter(|row| row["parent_id"] == target["id"])
            {
                child["parent_id"] = Value::Null;
            }
        }
        assert_eq!(
            referential_rows(&rt).await,
            expected,
            "{scenario}: only the targeted parent and its catalog-defined child effects may change"
        );
    }
    rt.shutdown().await;
}

#[tokio::test]
async fn approval_does_not_expose_sensitive_fields_but_trusts_code_with_non_sensitive_notes() {
    let f = fixture().await;
    approve(&f.rt, &["data_probe"]).await;
    for (label, token) in [("operator", &f.actors[0].token), ("admin", &f.rt.token)] {
        let (status, identity) =
            call(&f.rt, token, "data_probe", json!({"operation": "identity"})).await;
        assert_eq!(status, StatusCode::OK, "{label}: {identity}");
        let notes: BTreeSet<_> = identity["value"]["projects"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["internal_notes"].as_str().unwrap())
            .collect();
        assert_eq!(
            notes,
            BTreeSet::from(["internal-Alpha", "internal-Beta", "internal-Private"]),
            "{label}: backend is trusted with every non-sensitive column inside its declared table"
        );
        for transaction in [false, true] {
            for sql in [
                "SELECT confidential FROM support_actions.person",
                "SELECT display_name FROM support_actions.person WHERE confidential = 'sensitive-Alpha'",
                "SELECT to_jsonb(p) FROM support_actions.person p",
            ] {
                let (status, body) = call(
                    &f.rt,
                    token,
                    "data_probe",
                    json!({
                        "operation": "sql", "sql": sql, "transaction": transaction
                    }),
                )
                .await;
                assert_eq!(
                    status,
                    StatusCode::OK,
                    "{label}/tx={transaction}/{sql}: {body}"
                );
                assert!(
                    body["error"].is_string(),
                    "{label}/tx={transaction}/{sql}: {body}"
                );
                assert!(!body.to_string().contains("sensitive-Private"), "{body}");
            }
        }
        let (status, people) = call(
            &f.rt,
            token,
            "data_probe",
            json!({
                "operation": "collection", "entity": "person", "verb": "find"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{label}: {people}");
        let rows = people["value"]
            .as_array()
            .expect("approved local collection rows");
        assert_eq!(rows.len(), 3, "{label}: {people}");
        for person in rows {
            assert!(person.get("confidential").is_none(), "{label}: {person}");
        }
        let (status, body) = call(
            &f.rt,
            token,
            "data_probe",
            json!({
                "operation": "collection", "entity": "person", "verb": "find",
                "args": [{"confidential": "sensitive-Alpha"}]
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{label}: {body}");
        assert!(
            body["error"].is_string(),
            "{label}: sensitive predicate was accepted: {body}"
        );
    }
    f.rt.shutdown().await;
}

#[tokio::test]
async fn approved_workers_have_no_secrets_jobs_tools_subcalls_or_cross_app_data() {
    let f = fixture().await;
    f.rt.install_manifest(&json!({
        "appId": "support_archive", "name": "Support archive", "version": "1.0.0",
        "dataContract": [{"entityName": "records", "fields": [{"name": "label", "type": "text"}]}]
    }))
    .await;
    f.rt.create(
        "support_archive",
        "records",
        &json!({"label": "foreign canary"}),
    )
    .await;
    let (status, body) =
        f.rt.post_json(
            &format!("/api/v1/apps/{APP}/secrets"),
            &json!({"key": "SUPPORT_TOKEN", "value": "must-never-reach-approved-code"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    approve(&f.rt, &["data_probe", "add_interaction"]).await;
    let before = snapshot(&f.rt).await;
    for (label, token) in [("operator", &f.actors[0].token), ("admin", &f.rt.token)] {
        let (status, credentials) = call(
            &f.rt,
            token,
            "data_probe",
            json!({"operation": "credentials"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{label}: {credentials}");
        assert_eq!(
            credentials,
            json!({"value": {"context": null, "environment": null}}),
            "{label}"
        );
        for operation in ["job", "tool", "subcall", "selfAction", "crossApp"] {
            let (status, body) = call(
                &f.rt,
                token,
                "data_probe",
                json!({
                    "operation": operation,
                    "input": {"projectId": f.projects[0]["id"], "message": "subcall escaped"}
                }),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{label}/{operation}: {body}");
            let error = body["error"]
                .as_str()
                .unwrap_or_else(|| panic!("{label}/{operation} succeeded: {body}"));
            assert!(
                error.to_lowercase().contains("capability")
                    && error.to_lowercase().contains("denied"),
                "{label}/{operation} must be denied at the capability boundary: {body}"
            );
        }
        for transaction in [false, true] {
            for sql in [
                "SELECT label FROM support_archive.records",
                "SELECT email FROM rootcx_system.users",
            ] {
                let (status, body) = call(
                    &f.rt,
                    token,
                    "data_probe",
                    json!({
                        "operation": "sql", "sql": sql, "transaction": transaction
                    }),
                )
                .await;
                assert_eq!(
                    status,
                    StatusCode::OK,
                    "{label}/tx={transaction}/{sql}: {body}"
                );
                assert!(
                    body["error"].is_string(),
                    "{label}/tx={transaction}/{sql}: {body}"
                );
            }
        }
    }
    assert_eq!(
        snapshot(&f.rt).await,
        before,
        "forbidden capability changed local data"
    );
    f.rt.shutdown().await;
}

#[tokio::test]
async fn approved_executions_do_not_share_cached_results_across_calls_users_or_actions() {
    let f = fixture().await;
    let backend = br#"
let cached;
const read_scratch = async (_params, caller, ctx) => {
  try {
    if (!cached) cached = {
      actor: caller.userId,
      rows: (await ctx.sql("SELECT label FROM support_actions.scratch")).rows,
    };
    return { value: cached };
  } catch (error) { return { error: error.message }; }
};
serve({ rpc: { data_probe: read_scratch, assigned_projects: read_scratch } });
"#;
    let (status, body) =
        f.rt.deploy(APP, &harness::make_tar_gz(&[("index.ts", backend)]))
            .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // The same code runs behind two different approvals. Only data_probe may
    // read scratch; cached results must not bypass the other action's ceiling.
    approve(&f.rt, &["data_probe", "assigned_projects"]).await;
    let (status, scratch) =
        f.rt.get_json(&format!("/api/v1/apps/{APP}/collections/scratch"))
            .await;
    assert_eq!(status, StatusCode::OK, "{scratch}");
    let id = scratch[0]["id"].as_str().unwrap();
    for (execution, actor_index) in [0, 0, 1, 0].into_iter().enumerate() {
        let actor = &f.actors[actor_index];
        let label = format!("current result for execution {execution}");
        let (status, body) =
            f.rt.patch_json(
                &format!("/api/v1/apps/{APP}/collections/scratch/{id}"),
                &json!({"label": label}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "execution {execution}: {body}");
        let (status, result) = call(&f.rt, &actor.token, "data_probe", json!({})).await;
        assert_eq!(status, StatusCode::OK, "execution {execution}: {result}");
        assert_eq!(
            result,
            json!({"value": {"actor": actor.id, "rows": [[label]]}}),
            "execution {execution}: module cache retained another invocation's data or caller"
        );
        let (status, confined) = call(&f.rt, &actor.token, "assigned_projects", json!({})).await;
        assert_eq!(status, StatusCode::OK, "execution {execution}: {confined}");
        assert!(
            confined["error"].is_string() && confined.get("value").is_none(),
            "execution {execution}: another action reused cached scratch data: {confined}"
        );
    }
    f.rt.shutdown().await;
}

#[tokio::test]
async fn approved_execution_refuses_artifact_bytes_changed_after_approval() {
    let f = fixture().await;
    let review = approve(&f.rt, &["data_probe"]).await;
    let (status, before_tampering) = call(
        &f.rt,
        &f.actors[0].token,
        "data_probe",
        json!({"operation": "identity"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{before_tampering}");
    assert_eq!(
        before_tampering["value"]["projects"]
            .as_array()
            .unwrap()
            .len(),
        f.projects.len()
    );
    let before = snapshot(&f.rt).await;
    let entrypoint =
        f.rt.runtime
            .data_dir()
            .join("approved-backends")
            .join(review["revision"].as_str().unwrap())
            .join("index.ts");
    let original = std::fs::read(&entrypoint).unwrap();
    let original_permissions = std::fs::metadata(&entrypoint).unwrap().permissions();
    let mut writable = original_permissions.clone();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        writable.set_mode(writable.mode() | 0o200);
    }
    #[cfg(not(unix))]
    writable.set_readonly(false);
    // The fixture owner can alter its own snapshot without root. Restore its
    // permissions before invoking so rejection must concern bytes, not modes.
    std::fs::set_permissions(&entrypoint, writable.clone()).unwrap();
    let mut changed = original.clone();
    changed.extend_from_slice(b"\n// Changed after approval.\n");
    std::fs::write(&entrypoint, changed).unwrap();
    std::fs::set_permissions(&entrypoint, original_permissions.clone()).unwrap();
    for (index, actor) in f.actors.iter().enumerate() {
        let (status, body) = call(&f.rt, &actor.token, "data_probe", json!({
            "operation": "sql",
            "sql": "INSERT INTO support_actions.scratch (label) VALUES ('unreviewed artifact executed') RETURNING label"
        })).await;
        assert!(
            !status.is_success(),
            "actor {index}: changed artifact executed: {status} {body}"
        );
        assert!(
            body.to_string().to_lowercase().contains("digest"),
            "actor {index}: rejection must identify the artifact digest mismatch: {body}"
        );
        assert_eq!(
            snapshot(&f.rt).await,
            before,
            "actor {index}: changed artifact performed an approved write"
        );
    }
    std::fs::set_permissions(&entrypoint, writable).unwrap();
    std::fs::write(&entrypoint, original).unwrap();
    std::fs::set_permissions(&entrypoint, original_permissions).unwrap();
    f.rt.shutdown().await;
}

#[tokio::test]
async fn approval_and_role_revocation_take_effect_on_warm_workers_without_revoking_other_actions() {
    let f = fixture().await;
    approve(&f.rt, &["assigned_projects", "add_interaction"]).await;
    let (status, warm) = call(&f.rt, &f.actors[0].token, "assigned_projects", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{warm}");
    assert_eq!(warm["projects"].as_array().unwrap().len(), 1);
    for phase in ["role", "approval"] {
        let (status, body) = if phase == "role" {
            f.rt.post_json(
                "/api/v1/roles/revoke",
                &json!({
                    "userId": f.actors[0].id, "role": "support_operator"
                }),
            )
            .await
        } else {
            f.rt.delete_json(&format!("{APPROVALS}/assigned_projects"))
                .await
        };
        assert!(status.is_success(), "{phase}: {body}");
        let (status, denied) =
            call(&f.rt, &f.actors[0].token, "assigned_projects", json!({})).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{phase}: {denied}");
        let (status, other) = call(&f.rt, &f.actors[1].token, "assigned_projects", json!({})).await;
        if phase == "role" {
            assert_eq!(status, StatusCode::OK, "{phase}: {other}");
            assert_eq!(other["projects"].as_array().unwrap().len(), 2, "{other}");
            let (status, body) =
                f.rt.post_json(
                    "/api/v1/roles/assign",
                    &json!({
                        "userId": f.actors[0].id, "role": "support_operator"
                    }),
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{body}");
        } else {
            assert_eq!(status, StatusCode::FORBIDDEN, "{phase}: {other}");
            let (status, review) = f.rt.get_json(APPROVALS).await;
            assert_eq!(status, StatusCode::OK, "{review}");
            assert_eq!(entry(&review, "assigned_projects")["status"], "pending");
            assert_eq!(
                entry(&review, "assigned_projects")["approvalId"],
                Value::Null
            );
            assert_eq!(entry(&review, "add_interaction")["status"], "approved");
            let (status, body) = call(
                &f.rt,
                &f.actors[0].token,
                "add_interaction",
                json!({
                    "projectId": f.projects[0]["id"], "message": "other action stays approved"
                }),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body["author_id"], f.actors[0].id.to_string());
            approve(&f.rt, &["assigned_projects"]).await;
        }
        let (status, restored) =
            call(&f.rt, &f.actors[0].token, "assigned_projects", json!({})).await;
        assert_eq!(status, StatusCode::OK, "{phase}: {restored}");
        assert_eq!(restored["projects"], warm["projects"], "{phase}");
    }
    f.rt.shutdown().await;
}

#[tokio::test]
async fn deployment_manifest_and_installation_changes_require_fresh_approval() {
    let f = fixture().await;
    let mut before_cosmetic = None;
    for scenario in [
        "identical redeploy",
        "changed backend",
        "cosmetic manifest",
        "restore cosmetic manifest",
        "governance manifest",
        "uninstall",
    ] {
        approve(&f.rt, &["data_probe"]).await;
        let (status, old) = f.rt.get_json(APPROVALS).await;
        assert_eq!(status, StatusCode::OK, "{scenario}: {old}");
        let (status, old_worker) = call(
            &f.rt,
            &f.actors[0].token,
            "data_probe",
            json!({"operation": "identity"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{scenario}: {old_worker}");
        assert!(old_worker["value"]["workerId"].is_string(), "{old_worker}");
        match scenario {
            "identical redeploy" | "changed backend" => {
                let bytes = if scenario == "changed backend" {
                    String::from_utf8(BACKEND.to_vec())
                        .unwrap()
                        .replace(
                            "const build = \"original\"",
                            "const build = \"replacement\"",
                        )
                        .into_bytes()
                } else {
                    BACKEND.to_vec()
                };
                let (status, body) =
                    f.rt.deploy(APP, &harness::make_tar_gz(&[("index.ts", &bytes)]))
                        .await;
                assert_eq!(status, StatusCode::OK, "{scenario}: {body}");
            }
            "cosmetic manifest" => {
                before_cosmetic = Some(old.clone());
                let mut updated = manifest();
                updated["description"] = json!("Updated support desk display copy");
                f.rt.install_manifest(&updated).await;
            }
            "restore cosmetic manifest" => f.rt.install_manifest(&manifest()).await,
            "governance manifest" => {
                let mut updated = manifest();
                updated["actions"][2]["authority"]["data"]["scratch"] = json!(["read"]);
                f.rt.install_manifest(&updated).await;
            }
            "uninstall" => {
                let (status, body) = f.rt.delete_json(&format!("/api/v1/apps/{APP}")).await;
                assert!(status.is_success(), "{scenario}: {body}");
                let (status, body) = call(
                    &f.rt,
                    &f.actors[0].token,
                    "data_probe",
                    json!({"operation": "identity"}),
                )
                .await;
                assert!(
                    matches!(status, StatusCode::NOT_FOUND | StatusCode::FORBIDDEN),
                    "{scenario}: {body}"
                );
                f.rt.install_manifest(&manifest()).await;
                let (status, body) =
                    f.rt.patch_json(
                        "/api/v1/roles/support_operator",
                        &json!({"permissions": ACTIONS.map(key)}),
                    )
                    .await;
                assert_eq!(
                    status,
                    StatusCode::OK,
                    "restore caller grants after reinstall: {body}"
                );
                let (status, installed) = f.rt.get_json(APPROVALS).await;
                assert_eq!(status, StatusCode::OK, "{installed}");
                assert_ne!(installed["installationId"], old["installationId"]);
                assert_eq!(entry(&installed, "data_probe")["status"], "pending");
                let (status, body) =
                    f.rt.deploy(APP, &harness::make_tar_gz(&[("index.ts", BACKEND)]))
                        .await;
                assert_eq!(status, StatusCode::OK, "{body}");
            }
            _ => unreachable!(),
        }
        let (status, current) = f.rt.get_json(APPROVALS).await;
        assert_eq!(status, StatusCode::OK, "{scenario}: {current}");
        assert_ne!(
            current["revision"], old["revision"],
            "{scenario}: revision did not retire"
        );
        if matches!(
            scenario,
            "identical redeploy"
                | "cosmetic manifest"
                | "restore cosmetic manifest"
                | "governance manifest"
        ) {
            assert_eq!(current["backendDigest"], old["backendDigest"], "{scenario}");
        } else if scenario == "changed backend" {
            assert_ne!(current["backendDigest"], old["backendDigest"], "{scenario}");
        }
        if matches!(scenario, "governance manifest" | "uninstall") {
            assert_ne!(
                current["installationId"], old["installationId"],
                "{scenario}"
            );
        } else {
            assert_eq!(
                current["installationId"], old["installationId"],
                "{scenario}"
            );
        }
        assert_eq!(
            entry(&current, "data_probe")["status"],
            "pending",
            "{scenario}"
        );
        assert_eq!(
            entry(&current, "data_probe")["approvalId"],
            Value::Null,
            "{scenario}"
        );
        for token in [&f.actors[0].token, &f.rt.token] {
            let (status, body) =
                call(&f.rt, token, "data_probe", json!({"operation": "identity"})).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{scenario}: {body}");
        }
        let (status, body) =
            f.rt.post_json(&format!("{APPROVALS}/data_probe"), &binding(&old))
                .await;
        assert!(
            matches!(status, StatusCode::BAD_REQUEST | StatusCode::CONFLICT),
            "{scenario}: stale review approved replacement: {status} {body}"
        );
        if scenario == "restore cosmetic manifest" {
            let original = before_cosmetic.as_ref().unwrap();
            assert_eq!(
                current["installationId"], original["installationId"],
                "{scenario}"
            );
            assert_eq!(
                current["backendDigest"], original["backendDigest"],
                "{scenario}"
            );
            assert_ne!(
                current["revision"], original["revision"],
                "restoring the same manifest must not revive its retired revision"
            );
            let (status, body) =
                f.rt.post_json(&format!("{APPROVALS}/data_probe"), &binding(original))
                    .await;
            assert!(
                matches!(status, StatusCode::BAD_REQUEST | StatusCode::CONFLICT),
                "restored manifest accepted the pre-change approval binding: {status} {body}"
            );
        }
        let (status, body) =
            f.rt.post_json(&format!("{APPROVALS}/data_probe"), &binding(&current))
                .await;
        assert!(status.is_success(), "{scenario}: {status} {body}");
        let (status, approved) = f.rt.get_json(APPROVALS).await;
        assert_eq!(status, StatusCode::OK, "{approved}");
        assert_ne!(
            entry(&approved, "data_probe")["approvalId"],
            entry(&old, "data_probe")["approvalId"],
            "{scenario}"
        );
        let (status, fresh) = call(
            &f.rt,
            &f.actors[0].token,
            "data_probe",
            json!({"operation": "identity"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{scenario}: {fresh}");
        assert!(fresh["value"]["workerId"].is_string(), "{fresh}");
        assert_ne!(
            fresh["value"]["workerId"], old_worker["value"]["workerId"],
            "{scenario}: fresh approval revived the previous worker"
        );
        let expected_build = if matches!(scenario, "identical redeploy" | "uninstall") {
            "original"
        } else {
            "replacement"
        };
        assert_eq!(
            fresh["value"]["build"], expected_build,
            "{scenario}: {fresh}"
        );
        if scenario == "governance manifest" {
            let (status, body) = call(&f.rt, &f.actors[0].token, "data_probe", json!({
                "operation": "sql", "sql": "INSERT INTO support_actions.scratch (label) VALUES ('old ceiling')"
            })).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert!(
                body["error"].is_string(),
                "new approval reused old manifest ceiling: {body}"
            );
        }
    }
    f.rt.shutdown().await;
}

#[tokio::test]
async fn reapproval_cannot_revive_an_inflight_worker_with_retired_authority() {
    let f = fixture().await;
    for scenario in ["revoke", "identical redeploy"] {
        approve(&f.rt, &["data_probe"]).await;
        let mut gate = WorkerGate::start().await;
        let client = f.rt.client.clone();
        let url = f.rt.url(&format!("/api/v1/apps/{APP}/rpc"));
        let token = f.actors[0].token.clone();
        let request = json!({"method": "data_probe", "params": {
            "operation": "held", "gate": gate.url
        }});
        let old_call = tokio::spawn(async move {
            let response = client
                .post(url)
                .bearer_auth(token)
                .json(&request)
                .send()
                .await
                .unwrap();
            (
                response.status(),
                response.json::<Value>().await.unwrap_or(Value::Null),
            )
        });
        let old_worker = tokio::time::timeout(Duration::from_secs(10), gate.arrivals.recv())
            .await
            .expect("real Bun worker did not enter held action")
            .expect("gate closed");
        let (status, body) = if scenario == "revoke" {
            f.rt.delete_json(&format!("{APPROVALS}/data_probe")).await
        } else {
            f.rt.deploy(APP, &harness::make_tar_gz(&[("index.ts", BACKEND)]))
                .await
        };
        assert!(status.is_success(), "{scenario}: {body}");
        approve(&f.rt, &["data_probe"]).await;
        let before = snapshot(&f.rt).await;
        gate.release.notify_one();
        let (status, body) = tokio::time::timeout(Duration::from_secs(10), old_call)
            .await
            .expect("retired worker did not terminate or reject its next statement")
            .unwrap();
        drop(gate);
        assert!(
            !status.is_success() || body["error"].is_string(),
            "{scenario}: new approval revived old invocation: {status} {body}"
        );
        assert_eq!(
            snapshot(&f.rt).await,
            before,
            "{scenario}: retired worker committed data"
        );
        let (status, fresh) = call(
            &f.rt,
            &f.actors[0].token,
            "data_probe",
            json!({"operation": "identity"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{scenario}: {fresh}");
        assert!(fresh["value"]["workerId"].is_string(), "{fresh}");
        assert_ne!(
            fresh["value"]["workerId"], old_worker,
            "{scenario}: old process regained authority"
        );
    }
    f.rt.shutdown().await;
}

#[tokio::test]
async fn approval_and_assignment_revocation_wait_for_an_admitted_interaction_callback() {
    let f = fixture().await;
    for scenario in ["approval", "assignment"] {
        approve(&f.rt, &["atomic_probe"]).await;
        let before = snapshot(&f.rt).await;
        let mut gate = WorkerGate::start().await;
        let client = f.rt.client.clone();
        let url = f.rt.url(&format!("/api/v1/apps/{APP}/rpc"));
        let token = f.actors[0].token.clone();
        let message = format!("committed before {scenario} revocation");
        let params = json!({
            "projectId": f.projects[0]["id"], "message": message, "gate": gate.url
        });
        let callback = tokio::spawn(async move {
            let response = client
                .post(url)
                .bearer_auth(token)
                .json(&json!({"method": "atomic_probe", "params": params}))
                .send()
                .await
                .unwrap();
            (response.status(), response.json::<Value>().await.unwrap())
        });
        let pid: i32 = tokio::time::timeout(Duration::from_secs(10), gate.arrivals.recv())
            .await
            .expect("callback did not reach gate after its first insert")
            .expect("callback gate closed")
            .parse()
            .unwrap();

        let client = f.rt.client.clone();
        let token = f.rt.token.clone();
        let (method, path, body, target) = if scenario == "approval" {
            (
                Method::DELETE,
                format!("{APPROVALS}/atomic_probe"),
                None,
                "%action_approvals%",
            )
        } else {
            (
                Method::PATCH,
                format!(
                    "/api/v1/apps/{APP}/collections/assignment/{}",
                    f.assignments[0]["id"].as_str().unwrap()
                ),
                Some(json!({"revoked_at": "2099-01-01"})),
                "%assignment%",
            )
        };
        let url = f.rt.url(&path);
        let revocation = tokio::spawn(async move {
            let mut request = client.request(method, url).bearer_auth(token);
            if let Some(body) = body {
                request = request.json(&body);
            }
            let response = request.send().await.unwrap();
            (response.status(), response.json::<Value>().await.unwrap())
        });
        // A database lock wait proves overlap; an unfinished HTTP future alone
        // could simply mean that the server has not handled the request yet.
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                assert!(
                    !revocation.is_finished(),
                    "{scenario}: revocation returned before the admitted callback committed"
                );
                let blocked: bool = sqlx::query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM pg_stat_activity
                     WHERE datname = current_database() AND wait_event_type = 'Lock'
                       AND $1 = ANY(pg_blocking_pids(pid)) AND query LIKE $2)",
                )
                .bind(pid)
                .bind(target)
                .fetch_one(f.rt.pool())
                .await
                .unwrap();
                if blocked {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{scenario}: revoke did not wait on callback PID {pid}"));
        assert!(
            !revocation.is_finished(),
            "{scenario}: revoke escaped its lock wait"
        );
        assert_eq!(
            snapshot(&f.rt).await,
            before,
            "{scenario}: callback changes became visible before commit"
        );

        // Release first: awaiting revocation here would deadlock the test.
        gate.release.notify_one();
        let (status, recorded) = tokio::time::timeout(Duration::from_secs(10), callback)
            .await
            .expect("admitted callback did not finish")
            .unwrap();
        assert_eq!(status, StatusCode::OK, "{scenario}: {recorded}");
        assert!(
            recorded["value"]["id"].is_string(),
            "{scenario}: {recorded}"
        );
        let (status, revoked) = tokio::time::timeout(Duration::from_secs(10), revocation)
            .await
            .expect("revocation did not finish after callback commit")
            .unwrap();
        assert!(status.is_success(), "{scenario}: {status} {revoked}");
        drop(gate);

        let after = snapshot(&f.rt).await;
        assert_eq!(
            after["interaction"].as_array().unwrap().len(),
            before["interaction"].as_array().unwrap().len() + 1,
            "{scenario}"
        );
        assert_eq!(
            after["activity_log"].as_array().unwrap().len(),
            before["activity_log"].as_array().unwrap().len() + 1,
            "{scenario}"
        );
        let interaction = after["interaction"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == recorded["value"]["id"])
            .unwrap();
        assert_eq!(
            interaction_display(interaction),
            json!({
                "id": recorded["value"]["id"], "project_id": f.projects[0]["id"],
                "author_id": f.actors[0].id, "message": message
            }),
            "{scenario}"
        );
        let audit = after["activity_log"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["interaction_id"] == recorded["value"]["id"])
            .unwrap();
        assert_eq!(
            audit["author_id"],
            f.actors[0].id.to_string(),
            "{scenario}: {audit}"
        );
        let (status, denied) = call(
            &f.rt,
            &f.actors[0].token,
            "atomic_probe",
            json!({
                "projectId": f.projects[0]["id"], "message": "too late"
            }),
        )
        .await;
        if scenario == "approval" {
            assert_eq!(status, StatusCode::FORBIDDEN, "{scenario}: {denied}");
        } else {
            assert_eq!(status, StatusCode::OK, "{scenario}: {denied}");
            assert!(
                denied["error"]
                    .as_str()
                    .is_some_and(|e| e.contains("project is not assigned")),
                "{scenario}: {denied}"
            );
        }
        assert_eq!(
            snapshot(&f.rt).await,
            after,
            "{scenario}: post-revocation write committed"
        );
    }
    f.rt.shutdown().await;
}

#[tokio::test]
async fn approved_callback_transactions_commit_both_records_or_roll_back_every_write() {
    let f = fixture().await;
    approve(&f.rt, &["atomic_probe"]).await;
    for fault in ["none", "callback", "caughtSql"] {
        let before = snapshot(&f.rt).await;
        let (status, body) = call(&f.rt, &f.actors[0].token, "atomic_probe", json!({
            "projectId": f.projects[0]["id"], "message": format!("atomic {fault}"), "fault": fault
        })).await;
        assert_eq!(status, StatusCode::OK, "{fault}: {body}");
        let after = snapshot(&f.rt).await;
        if fault == "none" {
            assert!(body["value"]["id"].is_string(), "{body}");
            assert_eq!(
                after["interaction"].as_array().unwrap().len(),
                before["interaction"].as_array().unwrap().len() + 1
            );
            assert_eq!(
                after["activity_log"].as_array().unwrap().len(),
                before["activity_log"].as_array().unwrap().len() + 1
            );
            let audit = after["activity_log"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["interaction_id"] == body["value"]["id"])
                .unwrap();
            assert_eq!(audit["author_id"], f.actors[0].id.to_string(), "{audit}");
            assert_eq!(after["project"], before["project"]);
        } else {
            assert!(
                body["error"].is_string(),
                "{fault}: callback unexpectedly committed: {body}"
            );
            if fault == "callback" {
                assert!(
                    body["error"]
                        .as_str()
                        .unwrap()
                        .contains("follow-up validation failed"),
                    "{body}"
                );
            }
            assert_eq!(
                after, before,
                "{fault}: partial callback write escaped rollback"
            );
        }
    }
    f.rt.shutdown().await;
}
