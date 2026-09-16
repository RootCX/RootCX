use crate::harness::{self, TestRuntime};
use futures::FutureExt;
use reqwest::StatusCode;
use rootcx_core::RpcCaller;
use serde_json::{Value, json};
use uuid::Uuid;

async fn install(rt: &TestRuntime, app: &str, entity: &str) {
    rt.install_manifest(&json!({
        "appId": app, "name": app, "version": "1.0.0",
        "dataContract": [{"entityName": entity, "fields": [{"name": "name", "type": "text"}]}]
    }))
    .await;
}

async fn wait_name(rt: &TestRuntime, name: &str) -> bool {
    for _ in 0..200 {
        let found: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM consumer.results WHERE name = $1)")
                .bind(name)
                .fetch_one(rt.pool())
                .await
                .unwrap();
        if found {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    false
}

async fn rpc(rt: &TestRuntime, caller: RpcCaller, payload: Value) -> Value {
    rt.runtime
        .worker_manager()
        .rpc(
            "consumer",
            Uuid::new_v4().to_string(),
            "enqueue".into(),
            payload,
            Some(caller),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn worker_queue_preserves_delegated_ceiling_and_live_revocation() {
    let rt = TestRuntime::boot().await;
    let outcome = std::panic::AssertUnwindSafe(async {
        install(&rt, "consumer", "results").await;
        install(&rt, "provider", "records").await;
        let uid: Uuid = sqlx::query_scalar(
            "SELECT id FROM rootcx_system.users WHERE email = 'admin@test.local'",
        ).fetch_one(rt.pool()).await.unwrap();
        let (s, grant) = rt.post_json("/api/v1/cross-app/grants", &json!({
            "consumerApp": "consumer", "providerApp": "provider", "entity": "records",
            "actions": ["create"], "fields": ["name"], "writeFields": ["name"],
        })).await;
        assert_eq!(s, StatusCode::CREATED, "{grant}");
        let (s, body) = rt.post_json(
            &format!("/api/v1/cross-app/grants/{}/approve", grant["id"].as_str().unwrap()), &json!({}),
        ).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        let backend = br#"
async function attempt(ctx, label) {
  try { await ctx.remote("provider").collection("records").create({name: label}); return "allowed"; }
  catch (_) { return "denied"; }
}
serve({
  rpc: { enqueue: async (payload, _caller, ctx) => {
    const immediate = await attempt(ctx, payload.label + "-immediate");
    const id = await ctx.enqueueJob(payload);
    return { immediate, id };
  }},
  onJob: async (payload, _caller, ctx) => {
    const result = await attempt(ctx, payload.label + "-queued");
    await ctx.sql("INSERT INTO consumer.results (name) VALUES ($1)", [payload.label + "-" + result]);
  }
});
"#;
        let (s, body) = rt.deploy("consumer", &harness::make_tar_gz(&[("index.ts", backend)])).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        // Freeze delivery, not enqueue: exercise real IPC and durable authority before
        // changing live RBAC, without racing the scheduler.
        sqlx::raw_sql(
            "CREATE FUNCTION public.hold_test_job() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN NEW.vt := now() + interval '1 hour'; RETURN NEW; END $$;
         CREATE TRIGGER hold_test_job BEFORE INSERT ON pgmq.q_jobs
         FOR EACH ROW EXECUTE FUNCTION public.hold_test_job();",
        ).execute(rt.pool()).await.unwrap();
        let own = vec!["app:consumer:*".to_string()];
        for (label, perms, expected) in [
            ("stripped", Some(own.clone()), "denied"),
            ("allowed", Some(vec!["*".into()]), "allowed"),
            ("ordinary", None, "allowed"),
        ] {
            let result = rpc(&rt, RpcCaller {
                user_id: uid.to_string(), email: "admin@test.local".into(),
                effective_perms: perms, connection_id: None,
            }, json!({"label": label, "effective_perms": ["*"], "is_delegated": false})).await;
            assert_eq!(result["immediate"], expected, "{label}: {result}");
        }
        sqlx::query("UPDATE pgmq.q_jobs SET vt = now()").execute(rt.pool()).await.unwrap();
        rt.runtime.wake_scheduler();
        for name in ["stripped-denied", "allowed-allowed", "ordinary-allowed"] {
            assert!(wait_name(&rt, name).await, "queued outcome missing: {name}");
        }
        // Exercise the scheduler's trusted-envelope boundary for a distinct
        // principal. Raw agent IPC is separately denied, so the fixture models
        // Core-stamped authority without pretending payload JSON can supply it.
        let actor = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO rootcx_system.users (id, email, is_system, kind)
         VALUES ($1, 'queued-agent@localhost', true, 'agent')",
        ).bind(actor).execute(rt.pool()).await.unwrap();
        sqlx::query("INSERT INTO rootcx_system.rbac_roles (name, permissions) VALUES ('queue_actor', ARRAY['*'])")
            .execute(rt.pool()).await.unwrap();
        sqlx::query("INSERT INTO rootcx_system.rbac_assignments (user_id, role) VALUES ($1, 'queue_actor')")
            .bind(actor).execute(rt.pool()).await.unwrap();
        for change in ["role", "disabled", "delegation"] {
            sqlx::query("UPDATE rootcx_system.rbac_roles SET permissions = ARRAY['*'] WHERE name = 'queue_actor'")
                .execute(rt.pool()).await.unwrap();
            sqlx::query("UPDATE rootcx_system.users SET disabled_at = NULL WHERE id = $1")
                .bind(actor).execute(rt.pool()).await.unwrap();
            let delegation: Uuid = sqlx::query_scalar(
                "INSERT INTO rootcx_system.delegations (delegator_uid, delegatee_uid, trigger_type)
             VALUES ($1, $2, 'act_as') RETURNING id",
            ).bind(uid).bind(actor).fetch_one(rt.pool()).await.unwrap();
            let msg: i64 = sqlx::query_scalar("SELECT pgmq.send('jobs', $1)")
                .bind(json!({
                    "kind": "app", "app_id": "consumer", "user_id": uid,
                    "payload": {"label": format!("actor-{change}")},
                    "authority": {
                        "is_delegated": true, "principal_id": actor, "requires_delegation": true,
                        "effective_perms": ["*"], "connection_id": null,
                        "audit_actor_id": actor, "audit_delegator_id": uid
                    }
                })).fetch_one(rt.pool()).await.unwrap();
            match change {
                "role" => {
                    sqlx::query("UPDATE rootcx_system.rbac_roles SET permissions = $1 WHERE name = 'queue_actor'")
                        .bind(&own).execute(rt.pool()).await.unwrap();
                }
                "disabled" => {
                    sqlx::query("UPDATE rootcx_system.users SET disabled_at = now() WHERE id = $1")
                        .bind(actor).execute(rt.pool()).await.unwrap();
                }
                _ => {
                    sqlx::query("UPDATE rootcx_system.delegations SET revoked_at = now() WHERE delegatee_uid = $1")
                        .bind(actor).execute(rt.pool()).await.unwrap();
                }
            }
            sqlx::query("UPDATE pgmq.q_jobs SET vt = now() WHERE msg_id = $1")
                .bind(msg).execute(rt.pool()).await.unwrap();
            rt.runtime.wake_scheduler();
            if change == "role" {
                assert!(wait_name(&rt, "actor-role-denied").await, "revoked actor role must deny queued mutation");
            }
            let mut consumed = false;
            for _ in 0..200 {
                consumed = sqlx::query_scalar("SELECT NOT EXISTS(SELECT 1 FROM pgmq.q_jobs WHERE msg_id = $1)")
                    .bind(msg).fetch_one(rt.pool()).await.unwrap();
                if consumed { break; }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            assert!(consumed, "{change}: scheduler did not settle the message");
            sqlx::query("DELETE FROM rootcx_system.delegations WHERE id = $1")
                .bind(delegation).execute(rt.pool()).await.unwrap();
        }
        let result = rpc(&rt, RpcCaller {
            user_id: uid.to_string(), email: "admin@test.local".into(),
            effective_perms: Some(vec!["*".into()]), connection_id: None,
        }, json!({"label": "revoked"})).await;
        assert_eq!(result["immediate"], "allowed", "{result}");
        sqlx::query("DELETE FROM rootcx_system.rbac_assignments WHERE user_id = $1")
            .bind(uid).execute(rt.pool()).await.unwrap();
        sqlx::query("INSERT INTO rootcx_system.rbac_roles (name, permissions) VALUES ('queue_local', $1)")
            .bind(&own).execute(rt.pool()).await.unwrap();
        sqlx::query("INSERT INTO rootcx_system.rbac_assignments (user_id, role) VALUES ($1, 'queue_local')")
            .bind(uid).execute(rt.pool()).await.unwrap();
        sqlx::query("UPDATE pgmq.q_jobs SET vt = now()").execute(rt.pool()).await.unwrap();
        rt.runtime.wake_scheduler();
        assert!(wait_name(&rt, "revoked-denied").await, "revoked human permission must deny queued mutation");
        let names: Vec<String> = sqlx::query_scalar("SELECT name FROM provider.records ORDER BY name")
            .fetch_all(rt.pool()).await.unwrap();
        assert_eq!(names, ["allowed-immediate", "allowed-queued", "ordinary-immediate", "ordinary-queued", "revoked-immediate"]);
    })
    .catch_unwind()
    .await;
    rt.shutdown().await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
async fn app_payload_cannot_dispatch_or_mutate_native_workflow_execution() {
    let rt = TestRuntime::boot().await;
    let outcome = std::panic::AssertUnwindSafe(async {
        install(&rt, "consumer", "results").await;
        let graph = json!({
            "nodes": [
                {"id": "t", "kind": {"type": "trigger", "trigger": "manual"}, "params": {}, "position": [0,0]},
                {"id": "s", "kind": {"type": "control", "control": "set"}, "params": {"fields": {"done": true}}, "position": [1,0]}
            ], "edges": [{"from": "t", "to": "s", "fromOutput": 0}]
        });
        let (s, wf) = rt.post_json("/api/v1/workflows", &json!({"name": "queue-victim", "graph": graph})).await;
        assert_eq!(s, StatusCode::CREATED, "{wf}");
        let wf_id = wf["id"].as_str().unwrap();
        let (s, body) = rt.put_json(&format!("/api/v1/workflows/{wf_id}"), &json!({"enabled": true})).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        let uid: Uuid = sqlx::query_scalar(
            "SELECT id FROM rootcx_system.users WHERE email = 'admin@test.local'",
        ).fetch_one(rt.pool()).await.unwrap();
        let app: String = sqlx::query_scalar(
            "SELECT app_id FROM rootcx_system.workflows WHERE id = $1::uuid",
        ).bind(wf_id).fetch_one(rt.pool()).await.unwrap();
        let exec: Uuid = sqlx::query_scalar(
            "INSERT INTO rootcx_system.workflow_executions
         (id, workflow_id, app_id, run_as_user_id, graph, status)
         SELECT gen_random_uuid(), id, app_id, $2, graph, 'queued'
         FROM rootcx_system.workflows WHERE id = $1::uuid RETURNING id",
        ).bind(wf_id).bind(uid).fetch_one(rt.pool()).await.unwrap();
        let backend = br#"
serve({
  rpc: { enqueue: async (payload, _caller, ctx) => ({ id: await ctx.enqueueJob(payload) }) },
  onJob: async (payload, _caller, ctx) => {
    await ctx.sql("INSERT INTO consumer.results (name) VALUES ($1)", [payload.label]);
  }
});
"#;
        let (s, body) = rt.deploy("consumer", &harness::make_tar_gz(&[("index.ts", backend)])).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        for (label, payload) in [
            ("manual", json!({"action_type": "workflow", "manual": true, "execution_id": exec})),
            ("hook", json!({"_hook": true, "action_type": "workflow", "action_config": {"workflow_id": wf_id}})),
            ("cron", json!({"cron_id": Uuid::new_v4(), "workflow_id": wf_id})),
        ] {
            let mut payload = payload;
            payload["label"] = json!(label);
            payload["kind"] = json!("workflow_manual");
            rpc(&rt, RpcCaller {
                user_id: uid.to_string(), email: "admin@test.local".into(),
                effective_perms: None, connection_id: None,
            }, payload).await;
            assert!(wait_name(&rt, label).await, "{label}: ordinary app handler must receive payload");
        }
        let state: (String, i32, Option<i64>) = sqlx::query_as(
            "SELECT status, attempts, lease_msg_id FROM rootcx_system.workflow_executions WHERE id = $1",
        ).bind(exec).fetch_one(rt.pool()).await.unwrap();
        assert_eq!(state, ("queued".into(), 0, None));
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM rootcx_system.workflow_executions WHERE workflow_id = $1::uuid",
        ).bind(wf_id).fetch_one(rt.pool()).await.unwrap();
        assert_eq!(count, 1, "forged hook/cron must not create executions");
        // Corrupt/stale native bindings must be rejected before even the exhausted
        // delivery branch can mark someone else's execution failed.
        for mismatch in ["app", "user", "lease", "legacy"] {
            let mut envelope = json!({
                "kind": "workflow_manual", "app_id": app, "user_id": uid,
                "payload": {"execution_id": exec, "action_type": "workflow", "manual": true}
            });
            if mismatch == "app" { envelope["app_id"] = json!("consumer"); }
            if mismatch == "user" { envelope["user_id"] = json!(Uuid::new_v4()); }
            if mismatch == "legacy" { envelope.as_object_mut().unwrap().remove("kind"); }
            let msg: i64 = sqlx::query_scalar("SELECT pgmq.send('jobs', $1, 3600)")
                .bind(envelope).fetch_one(rt.pool()).await.unwrap();
            sqlx::query("UPDATE rootcx_system.workflow_executions SET lease_msg_id = $2 WHERE id = $1")
                .bind(exec).bind(if mismatch == "lease" { msg + 10000 } else { msg })
                .execute(rt.pool()).await.unwrap();
            sqlx::query("UPDATE pgmq.q_jobs SET read_ct = 6, vt = now() WHERE msg_id = $1")
                .bind(msg).execute(rt.pool()).await.unwrap();
            rt.runtime.wake_scheduler();
            let mut quarantined = false;
            for _ in 0..200 {
                quarantined = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM pgmq.q_jobs_dlq WHERE message->>'_dlq_msg_id' = $1)",
                ).bind(msg.to_string()).fetch_one(rt.pool()).await.unwrap();
                if quarantined { break; }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            assert!(quarantined, "{mismatch} did not reach quarantine");
            let state: (String, i32) = sqlx::query_as(
                "SELECT status, attempts FROM rootcx_system.workflow_executions WHERE id = $1",
            ).bind(exec).fetch_one(rt.pool()).await.unwrap();
            assert_eq!(state, ("queued".into(), 0), "{mismatch} changed victim");
        }
        // The authorized native route still dispatches a real workflow.
        let (s, body) = rt.post_json(&format!("/api/v1/workflows/{wf_id}/run"), &json!({})).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        let native_exec = body["executionId"].as_str().unwrap();
        let mut status = String::new();
        for _ in 0..200 {
            status = sqlx::query_scalar(
                "SELECT status FROM rootcx_system.workflow_executions WHERE id = $1::uuid",
            ).bind(native_exec).fetch_one(rt.pool()).await.unwrap();
            if status == "succeeded" { break; }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert_eq!(status, "succeeded");
    })
    .catch_unwind()
    .await;
    rt.shutdown().await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
async fn agent_raw_enqueue_without_invocation_scope_is_denied() {
    let rt = TestRuntime::boot().await;
    let outcome = std::panic::AssertUnwindSafe(async {
        rt.install("queueagent", "results").await;
        sqlx::query(
            "INSERT INTO rootcx_system.agents (app_id, name, config)
         VALUES ('queueagent', 'Queue agent', '{}')",
        )
        .execute(rt.pool())
        .await
        .unwrap();
        let backend = br#"
serve({ rpc: {
  enqueue: async (_payload, _caller, ctx) => {
    try { return { id: await ctx.enqueueJob({task_scope: ["*"]}) }; }
    catch (error) { return { denied: error.message }; }
  }
}});
"#;
        let (s, body) = rt
            .deploy(
                "queueagent",
                &harness::make_tar_gz(&[("index.ts", backend)]),
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{body}");
        let (s, body) = rt
            .post_json(
                "/api/v1/apps/queueagent/rpc",
                &json!({"method": "enqueue", "params": {}}),
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{body}");
        assert!(
            body["denied"]
                .as_str()
                .unwrap_or("")
                .contains("invocation-scoped"),
            "{body}"
        );
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pgmq.q_jobs WHERE message->>'app_id' = 'queueagent'",
        )
        .fetch_one(rt.pool())
        .await
        .unwrap();
        assert_eq!(count, 0);
    })
    .catch_unwind()
    .await;
    rt.shutdown().await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}
