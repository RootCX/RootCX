use crate::harness;

use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use uuid::Uuid;

async fn call(rt: &harness::TestRuntime, token: &str, params: Value) -> Value {
    let (status, body) = rt.request_as(
        Method::POST, "/api/v1/apps/consumer/rpc", token,
        Some(&json!({"method": "remote", "params": params})),
    ).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

#[tokio::test]
async fn worker_crud_confines_ownership_fields_and_rolls_back_when_audit_fails() {
    let rt = harness::TestRuntime::boot().await;
    rt.install("consumer", "events").await;
    rt.install_manifest(&json!({
        "appId": "provider", "name": "Provider", "version": "1.0.0",
        "dataContract": [{"entityName": "records", "fields": [
            {"name": "user_id", "type": "uuid", "owner": true},
            {"name": "name", "type": "text"},
            {"name": "private_note", "type": "text"},
            {"name": "secret", "type": "text", "sensitive": true}
        ]}]
    })).await;
    let token = rt.register_and_login("writer@test.local").await;
    let uid: Uuid = sqlx::query_scalar(
        "SELECT id FROM rootcx_system.users WHERE email = 'writer@test.local'",
    ).fetch_one(rt.pool()).await.unwrap();
    sqlx::query("DELETE FROM rootcx_system.rbac_assignments WHERE user_id = $1")
        .bind(uid).execute(rt.pool()).await.unwrap();
    sqlx::query(
        "INSERT INTO rootcx_system.rbac_roles (name, permissions)
         VALUES ('consumer_writer', ARRAY['app:consumer:invoke'])",
    ).execute(rt.pool()).await.unwrap();
    sqlx::query(
        "INSERT INTO rootcx_system.rbac_assignments (user_id, role) VALUES ($1, 'consumer_writer')",
    ).bind(uid).execute(rt.pool()).await.unwrap();
    let other = Uuid::new_v4();
    let other_record: Uuid = sqlx::query_scalar(
        "INSERT INTO provider.records (user_id, name) VALUES ($1, 'other') RETURNING id",
    ).bind(other).fetch_one(rt.pool()).await.unwrap();

    let backend = br#"
serve({ rpc: {
  remote: async (params, _caller, ctx) => {
    try {
      const c = ctx.remote("provider").collection("records");
      let result;
      if (params.op === "update") {
        if (params.objectForm) {
          result = await c.update({ ...params.data, id: params.id });
        } else {
          result = await c.update(params.id, params.data);
        }
      } else if (params.op === "delete") {
        result = await c.delete(params.id);
      } else {
        result = await c[params.op](params.data ?? {});
      }
      return { result };
    } catch (error) { return { denied: error.message }; }
  },
} });
"#;
    let (status, body) = rt.deploy(
        "consumer", &harness::make_tar_gz(&[("index.ts", backend)]),
    ).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, grant) = rt.post_json("/api/v1/cross-app/grants", &json!({
        "consumerApp": "consumer", "providerApp": "provider", "entity": "records",
        "actions": ["list", "read", "create", "update", "delete"],
        "fields": ["name"], "writeFields": ["name", "user_id"],
    })).await;
    assert_eq!(status, StatusCode::CREATED, "{grant}");
    let grant_id = grant["id"].as_str().unwrap();
    let pending = call(&rt, &token, json!({"op": "create", "data": {
        "user_id": uid, "name": "pending",
    }})).await;
    assert!(pending["denied"].is_string(), "{pending}");
    let (status, body) = rt.post_json(
        &format!("/api/v1/cross-app/grants/{grant_id}/approve"), &json!({}),
    ).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let created = call(&rt, &token, json!({"op": "create", "data": {
        "user_id": uid, "name": "mine",
    }})).await;
    assert_eq!(created["result"]["name"], "mine", "{created}");
    assert!(created["result"].get("user_id").is_none(), "{created}");
    let id = created["result"]["id"].as_str().unwrap();
    for object_form in [false, true] {
        let updated = call(&rt, &token, json!({
            "op": "update", "id": id, "data": {"name": "renamed"}, "objectForm": object_form,
        })).await;
        assert_eq!(updated["result"]["name"], "renamed", "object form {object_form}: {updated}");
    }

    for params in [
        json!({"op": "create", "data": {"user_id": other, "name": "forged"}}),
        json!({"op": "create", "data": {"user_id": uid, "name": "hidden", "secret": "x"}}),
        json!({"op": "create", "data": {"user_id": uid, "name": "private", "private_note": "x"}}),
        json!({"op": "create", "data": {"user_id": uid, "name": "fixed_id", "id": Uuid::new_v4()}}),
        json!({"op": "update", "id": id, "data": {"user_id": other}}),
        json!({"op": "update", "id": other_record, "data": {"name": "stolen"}}),
        json!({"op": "delete", "id": other_record}),
    ] {
        let result = call(&rt, &token, params.clone()).await;
        assert!(result["denied"].is_string(), "{params}: {result}");
    }
    let page = call(&rt, &token, json!({"op": "findPage"})).await;
    assert_eq!(page["result"]["total"], 1, "{page}");
    assert_eq!(page["result"]["data"][0]["name"], "renamed", "{page}");

    sqlx::raw_sql(
        "CREATE FUNCTION rootcx_system.fail_mutation_audit() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN
           IF NEW.outcome = 'success' AND NEW.action = 'create' THEN
             RAISE EXCEPTION 'injected audit outage';
           END IF;
           RETURN NEW;
         END $$;
         CREATE TRIGGER fail_mutation_audit BEFORE INSERT ON rootcx_system.cross_app_read_audit
         FOR EACH ROW EXECUTE FUNCTION rootcx_system.fail_mutation_audit();",
    ).execute(rt.pool()).await.unwrap();
    let failed = call(&rt, &token, json!({"op": "create", "data": {
        "user_id": uid, "name": "must_rollback",
    }})).await;
    assert!(failed["denied"].is_string(), "{failed}");
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM provider.records WHERE name = 'must_rollback'",
    ).fetch_one(rt.pool()).await.unwrap();
    assert_eq!(count, 0, "audit outage must roll back business mutation");

    let deleted = call(&rt, &token, json!({"op": "delete", "id": id})).await;
    assert_eq!(deleted["result"]["deleted"], true, "{deleted}");
    let (status, body) = rt.post_json(
        &format!("/api/v1/cross-app/grants/{grant_id}/revoke"), &json!({}),
    ).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let denied = call(&rt, &token, json!({"op": "delete", "id": other_record})).await;
    assert!(denied["denied"].is_string(), "{denied}");

    let remaining: Vec<String> = sqlx::query_scalar("SELECT name FROM provider.records")
        .fetch_all(rt.pool()).await.unwrap();
    assert_eq!(remaining, vec!["other"]);
    let audited: Vec<String> = sqlx::query_scalar(
        "SELECT action FROM rootcx_system.cross_app_read_audit
         WHERE outcome = 'success' AND action <> 'list' ORDER BY created_at",
    ).fetch_all(rt.pool()).await.unwrap();
    assert_eq!(audited, vec!["create", "update", "update", "delete"]);

    // A create-only grant may return its approved response projection, but
    // must not become standalone read, update or delete authority.
    sqlx::query("DROP TRIGGER fail_mutation_audit ON rootcx_system.cross_app_read_audit")
        .execute(rt.pool()).await.unwrap();
    let (status, grant) = rt.post_json("/api/v1/cross-app/grants", &json!({
        "consumerApp": "consumer", "providerApp": "provider", "entity": "records",
        "actions": ["create"], "fields": ["name"], "writeFields": ["name", "user_id"],
    })).await;
    assert_eq!(status, StatusCode::CREATED, "{grant}");
    let (status, approved) = rt.post_json(
        &format!("/api/v1/cross-app/grants/{}/approve", grant["id"].as_str().unwrap()),
        &json!({}),
    ).await;
    assert_eq!(status, StatusCode::OK, "{approved}");
    let create_only = call(&rt, &token, json!({"op": "create", "data": {
        "user_id": uid, "name": "create_only",
    }})).await;
    assert_eq!(create_only["result"]["name"], "create_only", "{create_only}");
    let created_id = &create_only["result"]["id"];
    for params in [
        json!({"op": "find"}),
        json!({"op": "findOne", "data": {"id": created_id}}),
        json!({"op": "update", "id": created_id, "data": {"name": "forbidden"}}),
        json!({"op": "delete", "id": created_id}),
    ] {
        let body = call(&rt, &token, params.clone()).await;
        assert!(body["denied"].is_string(), "{params}: {body}");
    }
    rt.shutdown().await;
}
