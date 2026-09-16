use crate::harness::{self, TestRuntime};
use reqwest::{Method, StatusCode};
use rootcx_core::tools::{Tool, ToolContext, query_data::QueryDataTool};
use serde_json::{Value, json};
use uuid::Uuid;

const VISIBLE: usize = 1_105;

struct Fixture {
    rt: TestRuntime,
    token: String,
    user: Uuid,
    invisible_local: Uuid,
}

async fn fixture() -> Fixture {
    let rt = TestRuntime::boot().await;
    for app in ["consumer", "provider"] {
        rt.install_manifest(&json!({
            "appId": app, "name": app, "version": "1.0.0",
            "dataContract": [{"entityName": "records", "fields": [
                {"name": "user_id", "type": "uuid", "owner": true},
                {"name": "ordinal", "type": "number"},
                {"name": "name", "type": "text"},
                {"name": "limit", "type": "number"},
                {"name": "data", "type": "json"},
                {"name": "ungranted", "type": "text"},
                {"name": "secret", "type": "text", "sensitive": true}
            ]}]
        }))
        .await;
    }
    let token = rt.register_and_login("collection-reader@test.local").await;
    let user: Uuid = sqlx::query_scalar(
        "SELECT id FROM rootcx_system.users WHERE email = 'collection-reader@test.local'",
    )
    .fetch_one(rt.pool())
    .await
    .unwrap();
    sqlx::query("DELETE FROM rootcx_system.rbac_assignments WHERE user_id = $1")
        .bind(user)
        .execute(rt.pool())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO rootcx_system.rbac_roles (name, permissions)
         VALUES ('collection_reader', ARRAY[
             'app:consumer:invoke', 'app:consumer:records.read.own',
             'app:consumer:records.create.own', 'app:consumer:records.update.own',
             'app:consumer:records.delete.own'
         ])",
    )
    .execute(rt.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO rootcx_system.rbac_assignments (user_id, role) VALUES ($1, 'collection_reader')",
    ).bind(user).execute(rt.pool()).await.unwrap();
    let other = Uuid::new_v4();
    for app in ["consumer", "provider"] {
        sqlx::query(&format!(
            "INSERT INTO {app}.records
             (user_id, ordinal, name, \"limit\", data, ungranted, secret, created_at)
             SELECT CASE WHEN n <= $2 THEN $1 ELSE $3 END, n, 'visible', 7,
                    '[1,2,3]'::jsonb, 'private', 'secret',
                    timestamptz '2026-01-01' + n * interval '1 second'
             FROM generate_series(1, $2 + 7) n",
        ))
        .bind(user)
        .bind(VISIBLE as i32)
        .bind(other)
        .execute(rt.pool())
        .await
        .unwrap();
    }
    let invisible_local =
        sqlx::query_scalar("SELECT id FROM consumer.records WHERE user_id = $1 LIMIT 1")
            .bind(other)
            .fetch_one(rt.pool())
            .await
            .unwrap();
    let backend = br#"
serve({ rpc: {
  collection: async (params, _caller, ctx) => {
    try {
      const c = params.scope === "remote"
        ? ctx.remote("provider").collection("records")
        : ctx.collection("records");
      return { result: await c[params.op](...(params.args ?? [])) };
    } catch (error) { return { error: error.message }; }
  },
} });
"#;
    let (status, body) = rt
        .deploy("consumer", &harness::make_tar_gz(&[("index.ts", backend)]))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, grant) = rt
        .post_json(
            "/api/v1/cross-app/grants",
            &json!({
                "consumerApp": "consumer", "providerApp": "provider", "entity": "records",
                "actions": ["list", "read"], "fields": ["ordinal", "name", "limit", "data"],
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{grant}");
    let (status, body) = rt
        .post_json(
            &format!(
                "/api/v1/cross-app/grants/{}/approve",
                grant["id"].as_str().unwrap()
            ),
            &json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    Fixture {
        rt,
        token,
        user,
        invisible_local,
    }
}

async fn call(f: &Fixture, scope: &str, op: &str, args: Value) -> (StatusCode, Value) {
    f.rt.request_as(
        Method::POST,
        "/api/v1/apps/consumer/rpc",
        &f.token,
        Some(&json!({"method": "collection", "params": {"scope": scope, "op": op, "args": args}})),
    )
    .await
}

#[tokio::test]
async fn full_reads_exceed_page_caps_without_widening_rows_or_remote_projection() {
    let f = fixture().await;
    for scope in ["local", "remote"] {
        // "limit" is an equality column here, never pagination metadata.
        let (status, body) = call(&f, scope, "find", json!([{"limit": 7}])).await;
        assert_eq!(status, StatusCode::OK, "{scope}: {body}");
        let rows = body["result"].as_array().expect("find returns an array");
        assert_eq!(rows.len(), VISIBLE, "{scope}: find silently truncated");
        for row in rows {
            assert!(
                row["ordinal"].as_f64().unwrap() <= VISIBLE as f64,
                "{scope}: {row}"
            );
            assert!(row.get("secret").is_none(), "{scope}: {row}");
            if scope == "remote" {
                assert!(row.get("ungranted").is_none(), "{row}");
                assert!(row.get("user_id").is_none(), "{row}");
            }
        }
        let (status, missing) = call(&f, scope, "findOne", json!([{"ordinal": 99_999}])).await;
        assert_eq!(status, StatusCode::OK, "{scope}: {missing}");
        assert_eq!(missing.get("result"), Some(&Value::Null), "{scope}: {missing}");
    }
    let count: i64 = sqlx::query_scalar(
        "SELECT row_count FROM rootcx_system.cross_app_read_audit
         WHERE consumer_app = 'consumer' AND action = 'list' AND outcome = 'success'
         ORDER BY created_at DESC LIMIT 1",
    )
    .fetch_one(f.rt.pool())
    .await
    .unwrap();
    assert_eq!(
        count, VISIBLE as i64,
        "array audit count must count records"
    );

    for target in ["consumer", "provider"] {
        let mut ctx = ToolContext {
            pool: f.rt.pool().clone(),
            core_bound_app_id: Some("consumer".into()),
            app_id: "consumer".into(),
            user_id: f.user,
            invoker_user_id: Some(f.user),
            permissions: vec![format!("app:{target}:records.read.own")],
            task_scope: None,
            args: json!({"app": target, "entity": "records"}),
            agent_dispatch: None,
            integration_caller: None,
            action_caller: None,
            stream_tx: None,
            idempotency_key: None,
        };
        let result = QueryDataTool.execute(&ctx).await.unwrap();
        let rows = result
            .as_array()
            .expect("query_data without options returns an array");
        assert_eq!(
            rows.len(),
            VISIBLE,
            "{target}: query_data silently truncated"
        );
        let ordinals: Vec<f64> = rows
            .iter()
            .map(|row| row["ordinal"].as_f64().unwrap())
            .collect();
        let descending: Vec<f64> = (1..=VISIBLE).rev().map(|n| n as f64).collect();
        assert_eq!(ordinals, descending, "{target}: newest rows first");
        if target == "provider" {
            assert!(rows.iter().all(|row| row.get("ungranted").is_none()));
        }
        ctx.args["offset"] = json!(VISIBLE);
        let page = QueryDataTool.execute(&ctx).await.unwrap();
        let expected_total = if target == "provider" { VISIBLE } else { 0 };
        assert_eq!(page["total"], expected_total, "{target}: {page}");
        assert_eq!(page["data"], json!([]), "{target}: {page}");
        if target == "consumer" {
            for (limit, expected_len) in [(json!(0), 1), (json!(1001), 1000), (json!("bad"), 100)] {
                ctx.args = json!({
                    "app": target, "entity": "records", "limit": limit,
                    "offset": -1, "order": "bad", "orderBy": "unknown"
                });
                let page = QueryDataTool.execute(&ctx).await.unwrap();
                assert_eq!(
                    page["data"].as_array().unwrap().len(),
                    expected_len,
                    "{page}"
                );
                assert_eq!(page["total"], VISIBLE, "{page}");
                assert_eq!(page["data"][0]["ordinal"].as_f64(), Some(VISIBLE as f64));
            }
        }
    }
    f.rt.shutdown().await;
}

#[tokio::test]
async fn local_and_remote_pages_share_bounds_totals_and_field_safety() {
    let f = fixture().await;
    for scope in ["local", "remote"] {
        for (options, expected) in [
            (json!({}), 100),
            (json!({"limit": 1}), 1),
            (json!({"limit": 1000}), 1000),
            (json!({"limit": 1000, "offset": 1000}), VISIBLE - 1000),
            (json!({"offset": VISIBLE}), 0),
        ] {
            let (status, body) = call(&f, scope, "findPage", json!([options])).await;
            assert_eq!(status, StatusCode::OK, "{scope}: {body}");
            assert_eq!(body["result"]["total"], VISIBLE, "{scope}: {body}");
            let rows = body["result"]["data"]
                .as_array()
                .expect("findPage returns a page");
            assert_eq!(rows.len(), expected, "{scope}");
            assert!(
                rows.iter()
                    .all(|row| row["ordinal"].as_f64().unwrap() <= VISIBLE as f64)
            );
            assert!(rows.iter().all(|row| row.get("secret").is_none()));
            if scope == "remote" {
                assert!(rows.iter().all(|row| row.get("ungranted").is_none()));
            }
        }
        for options in [
            json!({"limit": 0}),
            json!({"limit": 1001}),
            json!({"limit": "100"}),
            json!({"offset": -1}),
            json!({"where": {"unknown": 1}}),
            json!({"where": {"secret": "secret"}}),
            json!({"orderBy": "secret"}),
            json!({"ordinal": 1}),
        ] {
            let (_, body) = call(&f, scope, "findPage", json!([options.clone()])).await;
            assert!(body["error"].is_string(), "{scope}/{options}: {body}");
        }
        let (_, body) = call(&f, scope, "find", json!([{"unknown": 1}])).await;
        assert!(body["error"].is_string(), "{scope}: {body}");
        let (_, body) = call(&f, scope, "find", json!([{"secret": "secret"}])).await;
        if scope == "remote" {
            assert!(body["error"].is_string(), "{body}");
        } else {
            let rows = body["result"]
                .as_array()
                .expect("legacy local equality predicate");
            assert_eq!(rows.len(), VISIBLE);
            assert!(rows.iter().all(|row| row.get("secret").is_none()));
        }
    }
    // An allowed query-control-named column cannot conceal another forbidden field.
    let (_, body) = call(
        &f,
        "remote",
        "find",
        json!([{"limit": 7, "ungranted": "private"}]),
    )
    .await;
    assert!(body["error"].is_string(), "{body}");
    f.rt.shutdown().await;
}

#[tokio::test]
async fn local_mutations_preserve_update_forms_and_enforce_owned_rows() {
    let f = fixture().await;
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM consumer.records")
        .fetch_one(f.rt.pool())
        .await
        .unwrap();
    for id in [
        json!("not-a-uuid"),
        json!({"name": "visible"}),
        json!(f.invisible_local),
    ] {
        let (_, body) = call(&f, "local", "delete", json!([id.clone()])).await;
        assert!(body["error"].is_string(), "{id}: {body}");
        let after: i64 = sqlx::query_scalar("SELECT count(*) FROM consumer.records")
            .fetch_one(f.rt.pool())
            .await
            .unwrap();
        assert_eq!(after, before, "{id}: denied delete changed data");
    }
    let visible: Uuid =
        sqlx::query_scalar("SELECT id FROM consumer.records WHERE user_id = $1 LIMIT 1")
            .bind(f.user)
            .fetch_one(f.rt.pool())
            .await
            .unwrap();
    let (_, body) = call(&f, "local", "delete", json!([visible])).await;
    assert_eq!(body["result"], json!({"id": visible, "deleted": true}));
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM consumer.records WHERE id = $1)")
            .bind(visible)
            .fetch_one(f.rt.pool())
            .await
            .unwrap();
    assert!(!exists, "successful delete must remove the visible record");
    for method in ["create", "insert"] {
        let (status, body) = call(
            &f, "local", method, json!([{"user_id": f.user, "name": "created"}]),
        ).await;
        assert_eq!(status, StatusCode::OK, "{method}: {body}");
        let id = body["result"]["id"].as_str().expect("created record ID");
        assert_eq!(body["result"]["name"], "created", "{method}: {body}");
        for args in [
            json!([{"id": id, "name": "updated"}]),
            json!([id, {"name": "updated"}]),
        ] {
            let (_, updated) = call(&f, "local", "update", args.clone()).await;
            assert_eq!(updated["result"]["name"], "updated", "{method}/{args}: {updated}");
            let stored: String = sqlx::query_scalar(
                "SELECT name FROM consumer.records WHERE id = $1",
            ).bind(Uuid::parse_str(id).unwrap()).fetch_one(f.rt.pool()).await.unwrap();
            assert_eq!(stored, "updated", "{method}/{args}");
        }
    }
    for args in [
        json!([{"id": f.invisible_local, "name": "forbidden"}]),
        json!([f.invisible_local, {"name": "forbidden"}]),
    ] {
        let (_, body) = call(&f, "local", "update", args.clone()).await;
        assert!(body["error"].is_string(), "{args}: {body}");
    }
    let invisible_name: String = sqlx::query_scalar(
        "SELECT name FROM consumer.records WHERE id = $1",
    ).bind(f.invisible_local).fetch_one(f.rt.pool()).await.unwrap();
    assert_eq!(invisible_name, "visible", "denied updates must leave the other owner's row intact");
    f.rt.shutdown().await;
}
