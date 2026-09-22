use crate::harness;

use reqwest::StatusCode;
use rootcx_core::governance::{
    authority::intersect_permissions,
    cross_app::{authorize_cross_app_operation, authorize_cross_app_read},
    enforcement::{
        ContextState, InvocationContext, TIMEOUT_INTERACTIVE_MS,
        Audit, DataAccess,
    },
};
use rootcx_core::tools::{Tool, ToolContext, query_data::QueryDataTool};
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn delegated_grants_respect_task_scope_and_own_requires_an_owned_table() {
    let rt = harness::TestRuntime::boot().await;
    rt.install("consumer", "events").await;
    rt.install_manifest(&json!({
        "appId": "provider", "name": "provider", "version": "1.0.0",
        "dataContract": [
            {"entityName": "owned", "fields": [
                {"name": "user_id", "type": "uuid", "owner": true},
                {"name": "name", "type": "text"},
                {"name": "secret", "type": "text", "sensitive": true},
                {"name": "ungranted", "type": "text"}
            ]},
            {"entityName": "unowned", "fields": [
                {"name": "name", "type": "text"},
                {"name": "secret", "type": "text", "sensitive": true},
                {"name": "ungranted", "type": "text"}
            ]}
        ]
    }))
    .await;
    rt.create_user("delegated@test.local").await;
    let uid: Uuid = sqlx::query_scalar(
        "SELECT id FROM rootcx_system.users WHERE email = 'delegated@test.local'",
    )
    .fetch_one(rt.pool())
    .await
    .unwrap();
    sqlx::query("DELETE FROM rootcx_system.rbac_assignments WHERE user_id = $1")
        .bind(uid)
        .execute(rt.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO provider.owned (user_id, name) VALUES ($1, 'mine'), ($2, 'other')")
        .bind(uid)
        .bind(Uuid::new_v4())
        .execute(rt.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO provider.unowned (name) VALUES ('one'), ('two')")
        .execute(rt.pool())
        .await
        .unwrap();

    for entity in ["owned", "unowned"] {
        sqlx::query(&format!(
            "UPDATE provider.{entity} SET secret = 'private-secret', ungranted = 'private-ungranted'"
        ))
        .execute(rt.pool())
        .await
        .unwrap();
        let (status, grant) = rt
            .post_json(
                "/api/v1/cross-app/grants",
                &json!({
                    "consumerApp": "consumer", "providerApp": "provider",
                    "entity": entity, "fields": ["name"]
                }),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{grant}");
        let (status, approved) = rt
            .post_json(
                &format!(
                    "/api/v1/cross-app/grants/{}/approve",
                    grant["id"].as_str().unwrap()
                ),
                &json!({}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{approved}");
        let authority =
            authorize_cross_app_read(rt.pool(), "consumer", "provider", entity, "list", &[])
                .await
                .unwrap();
        let read = format!("app:provider:{entity}.read");
        let own = format!("{read}.own");
        let full = vec!["tool:query_data".into(), read.clone()];
        let stripped = intersect_permissions(&full, &["tool:query_data".into()]);
        let scoped_own = intersect_permissions(
            &["tool:query_data".into(), "app:provider:*".into()],
            &["tool:query_data".into(), own],
        );
        for (label, delegated, principal, permissions, expected) in [
            (
                "nondelegated grant supplies read",
                false,
                Some(uid),
                vec![],
                if entity == "owned" { 1 } else { 2 },
            ),
            ("delegated read", true, Some(uid), full, 2),
            ("task scope strips read", true, Some(uid), stripped, 0),
            ("empty intersection", true, Some(uid), vec![], 0),
            (
                "own scope",
                true,
                Some(uid),
                scoped_own,
                if entity == "owned" { 1 } else { 0 },
            ),
            ("no responsible principal", true, None, vec![read], 0),
        ] {
            let state = ContextState {
                user_id: principal,
                is_delegated: delegated,
                effective_perms: permissions.clone(),
                ..ContextState::default()
            };
            let mut tx = DataAccess::app("provider", &state, "delegation_test", TIMEOUT_INTERACTIVE_MS)
        .invoked_by(&InvocationContext::default())
        .audited(Audit { actor: principal, delegator: None })
        .across_apps(Some(&authority))
        .begin(rt.pool())
            .await
            .unwrap();
            let count: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM provider.{entity}"))
                .fetch_one(&mut *tx)
                .await
                .unwrap();
            if label == "own scope" && entity == "owned" {
                // The ordinary .own policy also permits this row. Check the
                // grant entry point explicitly so it cannot mask a broken
                // normalization of .read.own to the grant's read action.
                let allowed: bool = sqlx::query_scalar(
                    "SELECT rootcx_system.check_cross_app_access('app:provider:owned.read.own')",
                )
                .fetch_one(&mut *tx)
                .await
                .unwrap();
                assert!(
                    allowed,
                    "owned grant must accept the narrowed .own permission"
                );
            }
            tx.commit().await.unwrap();
            assert_eq!(count, expected, "{entity}: {label}");
            if matches!(
                label,
                "delegated read" | "task scope strips read" | "own scope"
            ) {
                let mut ctx = ToolContext {
                    pool: rt.pool().clone(),
                    core_bound_app_id: Some("consumer".into()),
                    app_id: "consumer".into(),
                    user_id: uid,
                    invoker_user_id: principal,
                    permissions,
                    task_scope: Some(state.effective_perms.clone()),
                    args: json!({}),
                    agent_dispatch: None,
                    integration_caller: None,
                    action_caller: None,
                    stream_tx: None,
                    idempotency_key: None,
                };
                for offset in [0, 100] {
                    ctx.args = json!({
                        "app": "provider", "entity": entity,
                        "where": {}, "limit": 1, "offset": offset,
                    });
                    let result = QueryDataTool
                        .execute(&ctx)
                        .await
                        .unwrap_or_else(|error| panic!("{entity}/{label}/{offset}: {error}"));
                    assert_eq!(
                        result["total"],
                        json!(expected),
                        "{entity}/{label}/{offset}: {result}"
                    );
                    let rows = result["data"].as_array().expect("query envelope");
                    let expected_len = if offset == 0 && expected > 0 { 1 } else { 0 };
                    assert_eq!(
                        rows.len(),
                        expected_len,
                        "{entity}/{label}/{offset}: {result}"
                    );
                    for row in rows {
                        let fields = row.as_object().unwrap();
                        assert_eq!(fields.len(), 4, "{row}");
                        for field in ["id", "created_at", "updated_at", "name"] {
                            assert!(fields.contains_key(field), "{row}");
                        }
                    }
                    assert!(!result.to_string().contains("private-"), "{result}");
                }
            }
        }
    }
    rt.shutdown().await;
}

#[tokio::test]
async fn mutation_only_permissions_allow_returning_without_widening_ownership_or_task_scope() {
    let rt = harness::TestRuntime::boot().await;
    rt.install("consumer", "events").await;
    rt.install_manifest(&json!({
        "appId": "provider", "name": "provider", "version": "1.0.0",
        "dataContract": [
            {"entityName": "owned", "fields": [
                {"name": "user_id", "type": "uuid", "owner": true},
                {"name": "name", "type": "text"}
            ]},
            {"entityName": "unowned", "fields": [
                {"name": "user_id", "type": "uuid"},
                {"name": "name", "type": "text"}
            ]}
        ]
    }))
    .await;
    rt.create_user("mutator@test.local").await;
    let uid: Uuid =
        sqlx::query_scalar("SELECT id FROM rootcx_system.users WHERE email = 'mutator@test.local'")
            .fetch_one(rt.pool())
            .await
            .unwrap();
    let other = Uuid::new_v4();
    sqlx::query("DELETE FROM rootcx_system.rbac_assignments WHERE user_id = $1")
        .bind(uid)
        .execute(rt.pool())
        .await
        .unwrap();

    for entity in ["owned", "unowned"] {
        sqlx::query(&format!(
            "INSERT INTO provider.{entity} (user_id, name) VALUES ($1, 'mine'), ($2, 'other')"
        ))
        .bind(uid)
        .bind(other)
        .execute(rt.pool())
        .await
        .unwrap();
        let (status, grant) = rt
            .post_json(
                "/api/v1/cross-app/grants",
                &json!({
                    "consumerApp": "consumer", "providerApp": "provider",
                    "entity": entity, "actions": ["create", "update", "delete"],
                    "fields": ["name"], "writeFields": ["name", "user_id"]
                }),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{grant}");
        let (status, approved) = rt
            .post_json(
                &format!(
                    "/api/v1/cross-app/grants/{}/approve",
                    grant["id"].as_str().unwrap()
                ),
                &json!({}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{approved}");
        assert!(
            authorize_cross_app_read(rt.pool(), "consumer", "provider", entity, "list", &[])
                .await
                .is_err(),
            "mutation grants must not supply standalone list authority",
        );

        for action in ["create", "update", "delete"] {
            let written = if action == "delete" {
                vec![]
            } else {
                vec!["name".into(), "user_id".into()]
            };
            let authority = authorize_cross_app_operation(
                rt.pool(),
                "consumer",
                "provider",
                entity,
                action,
                &[],
                &written,
            )
            .await
            .unwrap();
            let permission = format!("app:provider:{entity}.{action}");
            let stripped = intersect_permissions(
                &[permission.clone(), "tool:mutate_data".into()],
                &["tool:mutate_data".into()],
            );
            for (label, delegated, permissions) in [
                ("grant only", false, vec![]),
                ("operation only", true, vec![permission.clone()]),
                (
                    "own operation only",
                    true,
                    vec![format!("{permission}.own")],
                ),
                ("stripped task scope", true, stripped),
                (
                    "read is not mutation authority",
                    true,
                    vec![format!("app:provider:{entity}.read")],
                ),
            ] {
                let state = ContextState {
                    user_id: Some(uid),
                    is_delegated: delegated,
                    effective_perms: permissions,
                    ..ContextState::default()
                };
                let denied = matches!(
                    label,
                    "stripped task scope" | "read is not mutation authority"
                ) || (label == "own operation only" && entity == "unowned");
                let mut tx = DataAccess::app("provider", &state, "crud_delegation_test", TIMEOUT_INTERACTIVE_MS)
        .invoked_by(&InvocationContext::default())
        .audited(Audit { actor: Some(uid), delegator: None })
        .across_apps(Some(&authority))
        .begin(rt.pool())
                .await
                .unwrap();
                let sql = match action {
                    "create" => format!(
                        "INSERT INTO provider.{entity} (user_id, name) VALUES ($1, 'new') RETURNING id"
                    ),
                    "update" => format!(
                        "UPDATE provider.{entity} SET name = 'changed' WHERE user_id IS NOT NULL RETURNING id"
                    ),
                    "delete" => format!(
                        "DELETE FROM provider.{entity} WHERE user_id IS NOT NULL RETURNING id"
                    ),
                    _ => unreachable!(),
                };
                let mut query = sqlx::query_scalar::<_, Uuid>(&sql);
                if action == "create" {
                    query = query.bind(uid);
                }
                let result = query.fetch_all(&mut *tx).await;
                if denied && action == "create" {
                    let error =
                        result.expect_err("INSERT must reject the missing action permission");
                    assert_eq!(
                        error.as_database_error().and_then(|e| e.code()).as_deref(),
                        Some("42501"),
                        "{entity}/{action}/{label}: {error}"
                    );
                } else {
                    let ids =
                        result.unwrap_or_else(|error| panic!("{entity}/{action}/{label}: {error}"));
                    let expected = if denied {
                        0
                    } else if action == "create" || entity == "owned" {
                        1
                    } else {
                        2
                    };
                    assert_eq!(ids.len(), expected, "{entity}/{action}/{label}");
                }
                tx.rollback().await.unwrap();

                if entity == "owned"
                    && matches!(action, "create" | "update")
                    && matches!(label, "grant only" | "own operation only")
                {
                    let mut tx = DataAccess::app("provider", &state, "crud_owner_check", TIMEOUT_INTERACTIVE_MS)
        .invoked_by(&InvocationContext::default())
        .audited(Audit { actor: Some(uid), delegator: None })
        .across_apps(Some(&authority))
        .begin(rt.pool())
                    .await
                    .unwrap();
                    let sql = if action == "create" {
                        "INSERT INTO provider.owned (user_id, name) VALUES ($1, 'foreign') RETURNING id"
                    } else {
                        "UPDATE provider.owned SET user_id = $1 WHERE user_id = $2 RETURNING id"
                    };
                    let mut query = sqlx::query_scalar::<_, Uuid>(sql).bind(other);
                    if action == "update" {
                        query = query.bind(uid);
                    }
                    let error = query.fetch_all(&mut *tx).await.expect_err(
                        "WITH CHECK must reject creating or transferring a foreign-owned row",
                    );
                    assert_eq!(
                        error.as_database_error().and_then(|e| e.code()).as_deref(),
                        Some("42501"),
                        "{action}/{label}: {error}"
                    );
                    tx.rollback().await.unwrap();
                }
            }
        }
    }
    rt.shutdown().await;
}

#[tokio::test]
async fn agent_app_workers_cannot_bypass_tool_dispatch_with_raw_remote_ipc() {
    let rt = harness::TestRuntime::boot().await;
    rt.install("consumer", "events").await;
    rt.install_manifest(&json!({
        "appId": "provider", "name": "provider", "version": "1.0.0",
        "dataContract": [{"entityName": "records", "fields": [{"name": "name", "type": "text"}]}]
    }))
    .await;
    sqlx::query("INSERT INTO provider.records (name) VALUES ('unchanged')")
        .execute(rt.pool())
        .await
        .unwrap();
    let agent_uid = rootcx_core::extensions::agents::agent_user_id("consumer");
    sqlx::query(
        "INSERT INTO rootcx_system.users (id, email, is_system, kind)
         VALUES ($1, 'agent+consumer@localhost', true, 'agent')",
    )
    .bind(agent_uid)
    .execute(rt.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO rootcx_system.agents (app_id, name, config) VALUES ('consumer', 'consumer', '{}')",
    ).execute(rt.pool()).await.unwrap();

    // Bypass the SDK entirely. Capture the supervisor's wire reply on stdin;
    // the prelude also consumes stdin to service the enclosing HTTP RPC.
    let backend = br#"
let sequence = 0;
let buffer = "";
const pending = new Map();
process.stdin.on("data", (chunk) => {
  buffer += chunk.toString();
  let newline;
  while ((newline = buffer.indexOf("\n")) !== -1) {
    const line = buffer.slice(0, newline);
    buffer = buffer.slice(newline + 1);
    const message = JSON.parse(line);
    if (message.type === "collection_op_result" && pending.has(message.id)) {
      pending.get(message.id)(message);
      pending.delete(message.id);
    }
  }
});
serve({ rpc: {
  raw: async (params) => {
    const id = `raw_${++sequence}`;
    const message = {
      type: "remote_collection_op", id, provider_app: "provider",
      entity: "records", op: params.op, data: params.data
    };
    if (params.mode !== "omitted") {
      message.invocation_id = params.mode === "null" ? null : "forged-invocation";
    }
    return await new Promise((resolve) => {
      pending.set(id, resolve);
      process.stdout.write(JSON.stringify(message) + "\n");
    });
  }
} });
"#;
    let (status, deployed) = rt
        .deploy("consumer", &harness::make_tar_gz(&[("index.ts", backend)]))
        .await;
    assert_eq!(status, StatusCode::OK, "{deployed}");
    // Create the grant after deploy so it binds the active installation.
    let (status, grant) = rt
        .post_json(
            "/api/v1/cross-app/grants",
            &json!({
                "consumerApp": "consumer", "providerApp": "provider", "entity": "records",
                "actions": ["list", "read", "create", "update", "delete"],
                "fields": ["name"], "writeFields": ["name"]
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{grant}");
    let (status, approved) = rt
        .post_json(
            &format!(
                "/api/v1/cross-app/grants/{}/approve",
                grant["id"].as_str().unwrap()
            ),
            &json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{approved}");
    let record_id: Uuid = sqlx::query_scalar("SELECT id FROM provider.records")
        .fetch_one(rt.pool())
        .await
        .unwrap();
    // The admin caller has ample permissions and an active grant. Only the
    // agent-worker boundary should refuse these requests, even while idle
    // between agent invocations and with arbitrary invocation metadata.
    for mode in ["omitted", "null", "forged"] {
        for (op, data) in [
            ("find", json!({})),
            ("findOne", json!({"id": record_id})),
            ("create", json!({"name": "forged"})),
            (
                "update",
                json!({"id": record_id, "data": {"name": "forged"}}),
            ),
            ("delete", json!({"id": record_id})),
        ] {
            let (status, result) = rt
                .post_json(
                    "/api/v1/apps/consumer/rpc",
                    &json!({
                        "method": "raw", "params": {"mode": mode, "op": op, "data": data}
                    }),
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{mode}/{op}: {result}");
            assert_eq!(
                result["error"], "agents must use query_data or mutate_data for cross-app access",
                "{mode}/{op}: {result}"
            );
            assert!(
                result.get("result").is_none_or(serde_json::Value::is_null),
                "{result}"
            );
        }
    }
    let names: Vec<String> = sqlx::query_scalar("SELECT name FROM provider.records")
        .fetch_all(rt.pool())
        .await
        .unwrap();
    assert_eq!(names, vec!["unchanged"]);
    rt.shutdown().await;
}

#[tokio::test]
async fn raw_sql_cannot_leave_its_bound_app_even_with_platform_admin_permissions() {
    use rootcx_core::governance::enforcement::run_sql;

    let rt = harness::TestRuntime::boot().await;
    for app in ["consumer", "provider"] {
        rt.install_manifest(&json!({
            "appId": app, "name": app, "version": "1.0.0",
            "dataContract": [{"entityName": "records", "fields": [{"name": "name", "type": "text"}]}]
        })).await;
        sqlx::query(&format!(
            "INSERT INTO {app}.records (name) VALUES ('unchanged')"
        ))
        .execute(rt.pool())
        .await
        .unwrap();
    }
    let uid: Uuid =
        sqlx::query_scalar("SELECT id FROM rootcx_system.users WHERE email = 'admin@test.local'")
            .fetch_one(rt.pool())
            .await
            .unwrap();
    assert!(
        authorize_cross_app_read(rt.pool(), "consumer", "provider", "records", "list", &[])
            .await
            .is_err(),
        "fixture must have no cross-app grant",
    );

    for delegated in [false, true] {
        let state = ContextState {
            user_id: Some(uid),
            is_delegated: delegated,
            effective_perms: vec!["*".into()],
            ..ContextState::default()
        };
        for spoof in [
            "SET rootcx.human_data_request = '1'",
            "SELECT set_config('rootcx.human_data_request', '1', true)",
            "SELECT pg_catalog.set_config('rootcx.human_data_request', '1', false)",
        ] {
            let result = run_sql(rt.pool(), "consumer", &state, spoof, &[]).await;
            assert!(result.is_err(), "worker cannot forge human origin: {spoof}: {result:?}");
        }
        let local = run_sql(
            rt.pool(),
            "consumer",
            &state,
            "SELECT name FROM consumer.records",
            &[],
        )
        .await
        .unwrap();
        assert_eq!(
            local.rows,
            vec![vec![json!("unchanged")]],
            "delegated={delegated}"
        );
        for sql in [
            "SELECT name FROM provider.records",
            "UPDATE provider.records SET name = 'forged' RETURNING name",
            "DELETE FROM provider.records RETURNING name",
        ] {
            let result = run_sql(rt.pool(), "consumer", &state, sql, &[])
                .await
                .unwrap();
            assert!(
                result.rows.is_empty(),
                "delegated={delegated}, {sql}: {result:?}"
            );
        }
        let error = run_sql(
            rt.pool(),
            "consumer",
            &state,
            "INSERT INTO provider.records (name) VALUES ('forged') RETURNING name",
            &[],
        )
        .await
        .expect_err("foreign INSERT must fail RLS");
        assert!(
            error.contains("row-level security"),
            "delegated={delegated}: {error}"
        );
        let join = run_sql(
            rt.pool(),
            "consumer",
            &state,
            "SELECT p.name FROM consumer.records c LEFT JOIN provider.records p ON TRUE",
            &[],
        )
        .await
        .unwrap();
        assert_eq!(join.rows, vec![vec![json!(null)]], "delegated={delegated}");

        // HTTP/provider-bound operations retain ordinary provider permissions;
        // the confinement is the app context, not a blanket removal of RBAC.
        let provider = run_sql(
            rt.pool(),
            "provider",
            &state,
            "SELECT name FROM provider.records",
            &[],
        )
        .await
        .unwrap();
        assert_eq!(
            provider.rows,
            vec![vec![json!("unchanged")]],
            "delegated={delegated}"
        );
    }
    rt.shutdown().await;
}
