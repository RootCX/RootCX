//! PostgreSQL regressions for lifecycle serialization and atomic grant control.
use crate::harness;

use std::time::Duration;

use reqwest::{Method, StatusCode};
use rootcx_types::AppManifest;
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, PgPool};
use tokio::time::timeout;
use uuid::Uuid;

fn provider(version: &str) -> Value {
    json!({
        "appId": "provider", "name": "Provider", "version": version,
        "dataContract": [{"entityName": "records", "fields": [
            {"name": "name", "type": "text"}
        ]}]
    })
}

fn provider_with_new_field(version: &str) -> Value {
    let mut manifest = provider(version);
    manifest["dataContract"][0]["fields"].as_array_mut().unwrap().push(json!({
        "name": format!("revision_{}", version.replace('.', "_")), "type": "text"
    }));
    manifest
}

fn grant_request(consumer: &str) -> Value {
    json!({
        "consumerApp": consumer, "providerApp": "provider",
        "entity": "records", "fields": ["name"], "actions": ["list"]
    })
}

async fn generation(pool: &PgPool) -> (Uuid, i64) {
    sqlx::query_as(
        "SELECT id, generation FROM rootcx_system.app_installations
          WHERE app_id = 'provider' AND active",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

fn spawn_install(
    rt: &harness::TestRuntime,
    manifest: Value,
    uninstall: bool,
) -> tokio::task::JoinHandle<(StatusCode, Value)> {
    let client = rt.client.clone();
    let url = rt.url(if uninstall {
        "/api/v1/apps/provider"
    } else {
        "/api/v1/apps"
    });
    let token = rt.token.clone();
    tokio::spawn(async move {
        let response = client
            .request(
                if uninstall {
                    Method::DELETE
                } else {
                    Method::POST
                },
                url,
            )
            .bearer_auth(token)
            .json(&manifest)
            .send()
            .await
            .unwrap();
        (response.status(), response.json().await.unwrap())
    })
}

#[tokio::test]
async fn concurrent_update_and_uninstall_hold_the_complete_lifecycle_lock_without_pool_starvation()
{
    for uninstall in [false, true] {
        let rt = harness::TestRuntime::boot().await;
        rt.install_manifest(&provider("1.0.0")).await;
        let (_, initial_generation) = generation(rt.pool()).await;
        sqlx::raw_sql(
            "CREATE FUNCTION rootcx_system.pause_install() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN
             IF NEW.id = 'provider' AND NEW.manifest->>'version' = '2.0.0' THEN
                 PERFORM pg_advisory_xact_lock(707070);
             END IF;
             RETURN NEW;
         END $$;
         CREATE TRIGGER pause_install BEFORE UPDATE ON rootcx_system.apps
         FOR EACH ROW EXECUTE FUNCTION rootcx_system.pause_install();",
        )
        .execute(rt.pool())
        .await
        .unwrap();
        let mut barrier = PgConnection::connect_with(&rt.pool().connect_options())
            .await
            .unwrap();
        sqlx::query("SELECT pg_advisory_lock(707070)")
            .execute(&mut barrier)
            .await
            .unwrap();
        let first = spawn_install(&rt, provider_with_new_field("2.0.0"), false);
        timeout(Duration::from_secs(10), async {
            loop {
                let waiting: bool = sqlx::query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM pg_locks WHERE locktype = 'advisory'
                  AND objid = 707070 AND NOT granted)",
                )
                .fetch_one(&mut barrier)
                .await
                .unwrap();
                if waiting {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let active: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM rootcx_system.app_installations WHERE app_id = 'provider' AND active",
        ).fetch_one(&mut barrier).await.unwrap();
        assert_eq!(active, 0, "old generation must be revoked before new metadata is exposed");
        let second = spawn_install(&rt, provider_with_new_field("3.0.0"), uninstall);
        timeout(Duration::from_secs(5), async {
            loop {
                let waiting: bool = sqlx::query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE state = 'active'
                  AND wait_event = 'advisory' AND query = 'SELECT pg_advisory_lock(hashtext($1))')",
                )
                .fetch_one(&mut barrier)
                .await
                .unwrap();
                if waiting {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("second update must wait for the entire first installation");
        assert!(!second.is_finished());
        // Leave only the first install's in-flight SQL connection and one free pool
        // slot. The second install must not consume that slot waiting for its lock.
        let mut reserved = Vec::new();
        for _ in 0..rt.pool().options().get_max_connections() - 2 {
            reserved.push(rt.pool().acquire().await.unwrap());
        }
        timeout(
            Duration::from_secs(2),
            sqlx::query("SELECT 1").execute(rt.pool()),
        )
        .await
        .expect("lifecycle waiter exhausted the callback pool")
        .unwrap();
        drop(reserved);
        sqlx::query("SELECT pg_advisory_unlock(707070)")
            .execute(&mut barrier)
            .await
            .unwrap();
        for task in [first, second] {
            let (status, body) = timeout(Duration::from_secs(10), task)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(status, StatusCode::OK, "{body}");
        }
        if uninstall {
            let active: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM rootcx_system.app_installations WHERE app_id = 'provider' AND active",
        ).fetch_one(rt.pool()).await.unwrap();
            assert_eq!(active, 0);
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM rootcx_system.apps WHERE id = 'provider')",
            )
            .fetch_one(rt.pool())
            .await
            .unwrap();
            assert!(!exists);
        } else {
            let (_, final_generation) = generation(rt.pool()).await;
            assert_eq!(final_generation, initial_generation + 2);
            let visible: String = sqlx::query_scalar(
                "SELECT manifest->>'version' FROM rootcx_system.apps WHERE id = 'provider'",
            )
            .fetch_one(rt.pool())
            .await
            .unwrap();
            assert_eq!(visible, "3.0.0");
        }
        rt.shutdown().await;
    }
}

#[tokio::test]
async fn creation_validates_the_contract_after_waiting_for_lifecycle_and_lists_generations() {
    let rt = harness::TestRuntime::boot().await;
    rt.install("consumer", "events").await;
    rt.install_manifest(&provider("1.0.0")).await;
    let (old_id, old_generation) = generation(rt.pool()).await;
    let mut lifecycle = PgConnection::connect_with(&rt.pool().connect_options())
        .await
        .unwrap();
    sqlx::query("SELECT pg_advisory_lock(hashtext('provider'))")
        .execute(&mut lifecycle)
        .await
        .unwrap();

    let mut reserved = Vec::new();
    for _ in 0..rt.pool().options().get_max_connections() - 1 {
        reserved.push(rt.pool().acquire().await.unwrap());
    }
    let mut observer = PgConnection::connect_with(&rt.pool().connect_options())
        .await
        .unwrap();
    {
        let request = grant_request("consumer");
        let creating = rt.post_json("/api/v1/cross-app/grants", &request);
        tokio::pin!(creating);
        // Drive the HTTP future until the handler reaches its app-lock attempt.
        let waiting = async {
            timeout(Duration::from_secs(5), async {
            loop {
                let attempted: bool = sqlx::query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM pg_stat_activity
                      WHERE pid <> pg_backend_pid()
                        AND query LIKE '%advisory_xact_lock%' AND query NOT LIKE '%pg_stat_activity%')",
                ).fetch_one(&mut observer).await.unwrap();
                if attempted { break; }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }).await.unwrap();
        };
        tokio::select! {
            result = &mut creating => panic!("grant escaped the lifecycle lock: {result:?}"),
            _ = waiting => {}
        }
        timeout(
            Duration::from_secs(2),
            sqlx::query("SELECT 1").execute(rt.pool()),
        )
        .await
        .expect("grant waiter held the last pooled connection")
        .unwrap();
        // Publish a new contract and installation under the exact installer lock.
        // This deliberately pauses at the former metadata/generation race seam.
        let mut changed = provider("2.0.0");
        changed["dataContract"][0]["fields"][0]["sensitive"] = json!(true);
        let changed: AppManifest = serde_json::from_value(changed).unwrap();
        let mut tx = lifecycle.begin().await.unwrap();
        sqlx::query("UPDATE rootcx_system.apps SET manifest = $1 WHERE id = 'provider'")
            .bind(serde_json::to_value(changed).unwrap())
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("UPDATE rootcx_system.app_installations SET active = FALSE WHERE id = $1")
            .bind(old_id)
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("INSERT INTO rootcx_system.app_installations (app_id, generation) VALUES ('provider', $1)")
        .bind(old_generation + 1).execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
        sqlx::query("SELECT pg_advisory_unlock(hashtext('provider'))")
            .execute(&mut lifecycle)
            .await
            .unwrap();
        let (status, body) = timeout(Duration::from_secs(5), &mut creating)
            .await
            .unwrap();
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM rootcx_system.cross_app_collection_grants")
                .fetch_one(rt.pool())
                .await
                .unwrap();
        assert_eq!(
            count, 0,
            "stale readable snapshot must not attach to a new generation"
        );
    }

    let request = json!({
        "consumerApp": "consumer", "providerApp": "provider",
        "entity": "records", "fields": ["id"]
    });
    let (status, grant) = rt.post_json("/api/v1/cross-app/grants", &request).await;
    assert_eq!(status, StatusCode::CREATED, "{grant}");
    let (status, listed) = rt.get_json("/api/v1/cross-app/grants").await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["consumerGeneration"], grant["consumerGeneration"]);
    assert_eq!(listed[0]["providerGeneration"], json!(old_generation + 1));
    drop(reserved);
    drop(observer);
    drop(lifecycle);
    rt.shutdown().await;
}

#[tokio::test]
async fn grant_transition_does_not_lock_joined_installation_rows() {
    let rt = harness::TestRuntime::boot().await;
    rt.install("consumer", "events").await;
    rt.install_manifest(&provider("1.0.0")).await;
    let (status, grant) = rt
        .post_json("/api/v1/cross-app/grants", &grant_request("consumer"))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{grant}");
    let mut held = rt.pool().begin().await.unwrap();
    sqlx::query(
        "SELECT id FROM rootcx_system.app_installations WHERE app_id = 'provider' FOR UPDATE",
    )
    .execute(&mut *held)
    .await
    .unwrap();
    let path = format!(
        "/api/v1/cross-app/grants/{}/approve",
        grant["id"].as_str().unwrap()
    );
    let (status, approved) = timeout(Duration::from_secs(3), rt.post_json(&path, &json!({})))
        .await
        .expect("grant transition waited on an installation row: uninstall lock inversion");
    assert_eq!(status, StatusCode::OK, "{approved}");
    assert_eq!(approved["status"], "active");
    held.rollback().await.unwrap();
    rt.shutdown().await;
}

#[tokio::test]
async fn revoke_waits_for_governed_read_commit_and_rejects_saved_authority() {
    use rootcx_core::governance::{cross_app, enforcement};

    let rt = harness::TestRuntime::boot().await;
    rt.install("consumer", "events").await;
    rt.install_manifest(&provider("1.0.0")).await;
    rt.create("provider", "records", &json!({"name": "visible"}))
        .await;
    let (status, grant) = rt
        .post_json("/api/v1/cross-app/grants", &grant_request("consumer"))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{grant}");
    let grant_id = grant["id"].as_str().unwrap();
    let (status, approved) = rt
        .post_json(
            &format!("/api/v1/cross-app/grants/{grant_id}/approve"),
            &json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{approved}");
    let saved = cross_app::authorize_cross_app_read(
        rt.pool(),
        "consumer",
        "provider",
        "records",
        "list",
        &["name".into()],
    )
    .await
    .unwrap();
    rt.register_and_login("reader@test.local").await;
    let actor: Uuid =
        sqlx::query_scalar("SELECT id FROM rootcx_system.users WHERE email = 'reader@test.local'")
            .fetch_one(rt.pool())
            .await
            .unwrap();
    let state = enforcement::ContextState {
        user_id: Some(actor),
        // This user has no provider role; the application grant supplies access.
        is_delegated: false,
        effective_perms: vec![],
        connection_id: None,
        audit_actor_id: Some(actor),
        audit_delegator_id: None,
        public_execution: None,
    };
    let invocation = enforcement::InvocationContext::default();
    let mut read = enforcement::begin_app_tx_with_invocation_and_cross_app(
        rt.pool(),
        "provider",
        &state,
        &invocation,
        Some(actor),
        None,
        "concurrent_revoke_test",
        enforcement::TIMEOUT_INTERACTIVE_MS,
        Some(&saved),
    )
    .await
    .unwrap();
    let read_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *read)
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM provider.records")
        .fetch_one(&mut *read)
        .await
        .unwrap();
    assert_eq!(count, 1);

    let client = rt.client.clone();
    let token = rt.token.clone();
    let url = rt.url(&format!("/api/v1/cross-app/grants/{grant_id}/revoke"));
    let revoke = tokio::spawn(async move {
        let response = client
            .post(url)
            .bearer_auth(token)
            .json(&json!({"reason": "concurrent revocation"}))
            .send()
            .await
            .unwrap();
        (response.status(), response.json::<Value>().await.unwrap())
    });
    timeout(Duration::from_secs(5), async {
        loop {
            let blocked_by_read: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM pg_locks
                  WHERE locktype = 'advisory' AND classid = 42 AND NOT granted
                    AND $1 = ANY(pg_blocking_pids(pid)))",
            )
            .bind(read_pid)
            .fetch_one(rt.pool())
            .await
            .unwrap();
            if blocked_by_read {
                break;
            }
            assert!(
                !revoke.is_finished(),
                "revoke completed before the read released its lock"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("revoke never waited on the governed read's grant lock");
    assert!(!revoke.is_finished());
    // The read remains authorized until it commits and releases the same lock.
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM provider.records")
        .fetch_one(&mut *read)
        .await
        .unwrap();
    assert_eq!(count, 1);
    read.commit().await.unwrap();
    let (status, revoked) = timeout(Duration::from_secs(5), revoke)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{revoked}");
    assert_eq!(revoked["status"], "revoked");
    assert_eq!(revoked["version"], json!(saved.grant_version + 1));

    {
        let stale = enforcement::begin_app_tx_with_invocation_and_cross_app(
            rt.pool(),
            "provider",
            &state,
            &invocation,
            Some(actor),
            None,
            "stale_revoke_test",
            enforcement::TIMEOUT_INTERACTIVE_MS,
            Some(&saved),
        )
        .await;
        let error = match stale {
            Err(error) => error,
            Ok(tx) => {
                tx.rollback().await.unwrap();
                panic!("saved authority opened a provider transaction after revocation");
            }
        };
        assert!(
            error
                .to_string()
                .contains("authorization changed before provider read"),
            "{error}"
        );
    }
    assert!(
        cross_app::authorize_cross_app_read(
            rt.pool(),
            "consumer",
            "provider",
            "records",
            "list",
            &["name".into()],
        )
        .await
        .is_err()
    );
    rt.shutdown().await;
}

#[tokio::test]
async fn failed_update_stays_revoked_and_retry_creates_a_fresh_generation() {
    let rt = harness::TestRuntime::boot().await;
    rt.install("consumer", "events").await;
    rt.install_manifest(&provider("1.0.0")).await;
    let (old_id, old_generation) = generation(rt.pool()).await;
    let (status, grant) = rt
        .post_json("/api/v1/cross-app/grants", &grant_request("consumer"))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{grant}");
    let path = format!(
        "/api/v1/cross-app/grants/{}/approve",
        grant["id"].as_str().unwrap()
    );
    let (status, body) = rt.post_json(&path, &json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    sqlx::raw_sql(
        "CREATE FUNCTION rootcx_system.fail_install() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN
             IF NEW.id = 'provider' AND NEW.manifest->>'version' = '2.0.0' THEN
                 RAISE EXCEPTION 'injected installation failure';
             END IF;
             RETURN NEW;
         END $$;
         CREATE TRIGGER fail_install BEFORE UPDATE ON rootcx_system.apps
         FOR EACH ROW EXECUTE FUNCTION rootcx_system.fail_install();",
    )
    .execute(rt.pool())
    .await
    .unwrap();
    let (status, error) = rt.post_json("/api/v1/apps", &provider_with_new_field("2.0.0")).await;
    assert_eq!(status, StatusCode::CONFLICT, "{error}");
    assert!(
        error
            .to_string()
            .contains("previous cross-app authority remains revoked"),
        "{error}"
    );
    let active: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rootcx_system.app_installations WHERE app_id = 'provider' AND active",
    )
    .fetch_one(rt.pool())
    .await
    .unwrap();
    assert_eq!(active, 0);
    let path = format!("/api/v1/cross-app/grants/{}", grant["id"].as_str().unwrap());
    let (status, grant) = rt.get_json(&path).await;
    assert_eq!(status, StatusCode::OK, "{grant}");
    assert_eq!(grant["status"], "revoked");

    sqlx::query("DROP TRIGGER fail_install ON rootcx_system.apps")
        .execute(rt.pool())
        .await
        .unwrap();
    // Also cover failure after metadata was saved but before callbacks finished.
    let partial: AppManifest = serde_json::from_value(provider_with_new_field("2.0.0")).unwrap();
    sqlx::query("UPDATE rootcx_system.apps SET manifest = $1 WHERE id = 'provider'")
        .bind(serde_json::to_value(partial).unwrap())
        .execute(rt.pool())
        .await
        .unwrap();
    // Recovery must still run even when the retry only adds cosmetic metadata
    // to the partially exposed contract.
    let mut retry = provider_with_new_field("2.0.0");
    retry["description"] = json!("Retried deployment");
    rt.install_manifest(&retry).await;
    let (new_id, new_generation) = generation(rt.pool()).await;
    assert_ne!(new_id, old_id);
    assert_eq!(new_generation, old_generation + 1);
    let (status, grant) = rt.get_json(&path).await;
    assert_eq!(status, StatusCode::OK, "{grant}");
    assert_eq!(grant["status"], "revoked");
    rt.shutdown().await;
}

#[tokio::test]
async fn cosmetic_manifest_updates_preserve_approved_authority_and_history() {
    use rootcx_core::governance::cross_app;

    let rt = harness::TestRuntime::boot().await;
    rt.install("consumer", "events").await;
    let mut manifest = provider("1.0.0");
    rt.install_manifest(&manifest).await;
    let original_generation = generation(rt.pool()).await;
    let (status, grant) = rt.post_json("/api/v1/cross-app/grants", &grant_request("consumer")).await;
    assert_eq!(status, StatusCode::CREATED, "{grant}");
    let path = format!("/api/v1/cross-app/grants/{}", grant["id"].as_str().unwrap());
    let (status, approved) = rt.post_json(&format!("{path}/approve"), &json!({})).await;
    assert_eq!(status, StatusCode::OK, "{approved}");
    let (status, history) = rt.get_json(&format!("{path}/audit")).await;
    assert_eq!(status, StatusCode::OK, "{history}");
    let saved = cross_app::authorize_cross_app_read(
        rt.pool(), "consumer", "provider", "records", "list", &["name".into()],
    ).await.unwrap();
    let editor_token = rt.register_and_login("cosmetic-editor@test.local").await;
    let editor: Uuid = sqlx::query_scalar(
        "SELECT id FROM rootcx_system.users WHERE email = 'cosmetic-editor@test.local'",
    ).fetch_one(rt.pool()).await.unwrap();
    sqlx::query("INSERT INTO rootcx_system.rbac_assignments (user_id, role) VALUES ($1, 'admin')")
        .bind(editor).execute(rt.pool()).await.unwrap();

    for (key, value) in [
        ("name", json!("Renamed provider")),
        ("version", json!("2.0.0")),
        ("description", json!("Updated description")),
        ("icon", json!("https://example.com/icon.svg")),
        ("icon", json!(null)),
    ] {
        manifest[key] = value.clone();
        let (status, body) = rt.request_as(
            Method::POST, "/api/v1/apps", &editor_token, Some(&manifest),
        ).await;
        assert_eq!(status, StatusCode::OK, "{key}={value}: {body}");
        assert_eq!(generation(rt.pool()).await, original_generation, "{key}");
        let (status, current) = rt.get_json(&path).await;
        assert_eq!(status, StatusCode::OK, "{key}: {current}");
        assert_eq!(current, approved, "{key}: approval must remain unchanged");
        let (status, current_history) = rt.get_json(&format!("{path}/audit")).await;
        assert_eq!(status, StatusCode::OK, "{key}: {current_history}");
        assert_eq!(current_history, history, "{key}");
        let stored: Value = sqlx::query_scalar(
            "SELECT manifest FROM rootcx_system.apps WHERE id = 'provider'",
        ).fetch_one(rt.pool()).await.unwrap();
        assert_eq!(stored[key], value, "{key}: metadata must be saved");
        let assigned: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM rootcx_system.rbac_assignments
              WHERE user_id = $1 AND role = 'app:provider:admin')",
        ).bind(editor).fetch_one(rt.pool()).await.unwrap();
        assert!(!assigned, "{key}: cosmetic editing must not change app ownership");
        let mut tx = rt.pool().begin().await.unwrap();
        cross_app::lock_cross_app_read_in_tx(&mut tx, &saved).await.unwrap();
        tx.rollback().await.unwrap();
    }
    rt.shutdown().await;
}

#[tokio::test]
async fn contract_updates_revoke_approved_authority_even_with_cosmetic_changes() {
    use rootcx_core::governance::cross_app;

    let rt = harness::TestRuntime::boot().await;
    rt.install("consumer", "events").await;
    let mut manifest = provider("1.0.0");
    rt.install_manifest(&manifest).await;
    for change in ["new_field", "permissions", "sensitive_field"] {
        let original_generation = generation(rt.pool()).await;
        let (status, grant) = rt.post_json("/api/v1/cross-app/grants", &grant_request("consumer")).await;
        assert_eq!(status, StatusCode::CREATED, "{change}: {grant}");
        let path = format!("/api/v1/cross-app/grants/{}", grant["id"].as_str().unwrap());
        let (status, approved) = rt.post_json(&format!("{path}/approve"), &json!({})).await;
        assert_eq!(status, StatusCode::OK, "{change}: {approved}");
        let saved = cross_app::authorize_cross_app_read(
            rt.pool(), "consumer", "provider", "records", "list", &["name".into()],
        ).await.unwrap();
        match change {
            "new_field" => manifest = provider_with_new_field("2.0.0"),
            "permissions" => manifest["permissions"] = json!({
                "permissions": [{"key": "app:provider:records.export"}]
            }),
            "sensitive_field" => {
                manifest["dataContract"][0]["fields"][0]["sensitive"] = json!(true);
            }
            _ => unreachable!(),
        }
        manifest["description"] = json!(change);
        rt.install_manifest(&manifest).await;
        let updated_generation = generation(rt.pool()).await;
        assert_ne!(updated_generation.0, original_generation.0, "{change}");
        assert_eq!(updated_generation.1, original_generation.1 + 1, "{change}");
        let (status, revoked) = rt.get_json(&path).await;
        assert_eq!(status, StatusCode::OK, "{change}: {revoked}");
        assert_eq!(revoked["status"], "revoked", "{change}");
        let mut tx = rt.pool().begin().await.unwrap();
        assert!(cross_app::lock_cross_app_read_in_tx(&mut tx, &saved).await.is_err(), "{change}");
        tx.rollback().await.unwrap();
        let (status, history) = rt.get_json(&format!("{path}/audit")).await;
        assert_eq!(status, StatusCode::OK, "{change}: {history}");
        assert_eq!(history.as_array().unwrap().iter()
            .filter(|event| event["operation"] == "auto_revoked").count(), 1, "{change}");
    }
    rt.shutdown().await;
}

#[tokio::test]
async fn workflow_delete_rolls_back_on_audit_failure_and_can_be_retried() {
    let rt = harness::TestRuntime::boot().await;
    rt.install_manifest(&provider("1.0.0")).await;
    let (status, workflow) = rt
        .post_json("/api/v1/workflows", &json!({"name": "Reader"}))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{workflow}");
    let id = workflow["id"].as_str().unwrap();
    let consumer = format!("wf-{id}");
    let (status, grant) = rt
        .post_json("/api/v1/cross-app/grants", &grant_request(&consumer))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{grant}");
    let grant_id = grant["id"].as_str().unwrap();
    let (status, body) = rt
        .post_json(
            &format!("/api/v1/cross-app/grants/{grant_id}/approve"),
            &json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    sqlx::raw_sql(
        "CREATE FUNCTION rootcx_system.reject_lifecycle_audit() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN
             IF NEW.operation = 'auto_revoked' THEN RAISE EXCEPTION 'injected audit failure'; END IF;
             RETURN NEW;
         END $$;
         CREATE TRIGGER reject_lifecycle_audit BEFORE INSERT ON rootcx_system.cross_app_grant_audit
         FOR EACH ROW EXECUTE FUNCTION rootcx_system.reject_lifecycle_audit();",
    ).execute(rt.pool()).await.unwrap();
    let path = format!("/api/v1/workflows/{id}");
    assert_eq!(rt.delete(&path).await, StatusCode::INTERNAL_SERVER_ERROR);
    let (status, workflow) = rt.get_json(&path).await;
    assert_eq!(status, StatusCode::OK, "{workflow}");
    let active: bool =
        sqlx::query_scalar("SELECT active FROM rootcx_system.app_installations WHERE app_id = $1")
            .bind(&consumer)
            .fetch_one(rt.pool())
            .await
            .unwrap();
    assert!(active, "failed deletion must roll back deactivation");
    let (status, grant) = rt
        .get_json(&format!("/api/v1/cross-app/grants/{grant_id}"))
        .await;
    assert_eq!(status, StatusCode::OK, "{grant}");
    assert_eq!(grant["status"], "active");

    sqlx::query("DROP TRIGGER reject_lifecycle_audit ON rootcx_system.cross_app_grant_audit")
        .execute(rt.pool())
        .await
        .unwrap();
    assert_eq!(rt.delete(&path).await, StatusCode::OK);
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM rootcx_system.apps WHERE id = $1)")
            .bind(&consumer)
            .fetch_one(rt.pool())
            .await
            .unwrap();
    assert!(!exists);
    let (status, history) = rt
        .get_json(&format!("/api/v1/cross-app/grants/{grant_id}/audit"))
        .await;
    assert_eq!(status, StatusCode::OK, "{history}");
    assert_eq!(
        history
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["operation"] == "auto_revoked")
            .count(),
        1
    );
    let (status, grant) = rt
        .get_json(&format!("/api/v1/cross-app/grants/{grant_id}"))
        .await;
    assert_eq!(status, StatusCode::OK, "{grant}");
    assert_eq!(grant["status"], "revoked");
    rt.shutdown().await;
}

#[tokio::test]
async fn crud_grants_freeze_independent_scopes_and_authorize_only_the_bound_operation() {
    use rootcx_core::governance::cross_app;

    let rt = harness::TestRuntime::boot().await;
    rt.install("consumer", "events").await;
    let mut manifest = provider("1.0.0");
    manifest["dataContract"][0]["fields"] = json!([
        {"name": "name", "type": "text"},
        {"name": "status", "type": "text"},
        {"name": "secret", "type": "text", "sensitive": true}
    ]);
    rt.install_manifest(&manifest).await;
    let request = json!({
        "consumerApp": "consumer", "providerApp": "provider", "entity": "records",
        "actions": ["create", "update", "delete"],
        "fields": ["name"], "writeFields": ["status", "status"]
    });
    for (key, value) in [
        ("writeFields", Value::Null),
        ("writeFields", json!([])),
        ("writeFields", json!(["id"])),
        ("writeFields", json!(["created_at"])),
        ("writeFields", json!(["updated_at"])),
        ("writeFields", json!(["secret"])),
        ("writeFields", json!(["missing"])),
        ("fields", json!(["secret"])),
        ("actions", json!(["delete"])),
        ("actions", json!(["upsert"])),
    ] {
        let mut invalid = request.clone();
        invalid[key] = value;
        let (status, body) = rt.post_json("/api/v1/cross-app/grants", &invalid).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{invalid}: {body}");
    }
    let (status, grant) = rt.post_json("/api/v1/cross-app/grants", &request).await;
    assert_eq!(status, StatusCode::CREATED, "{grant}");
    assert_eq!(grant["status"], "pending");
    assert_eq!(grant["writeFieldSnapshot"], json!(["status"]));
    let readable = grant["fieldSnapshot"].as_array().unwrap();
    assert!(readable.contains(&json!("name")));
    assert!(readable.contains(&json!("id")));
    assert!(!readable.contains(&json!("status")));
    assert!(!readable.contains(&json!("secret")));
    assert!(
        cross_app::authorize_cross_app_operation(
            rt.pool(),
            "consumer",
            "provider",
            "records",
            "create",
            &["name".into()],
            &["status".into()],
        )
        .await
        .is_err(),
        "pending writable grants must not authorize mutations"
    );

    let grant_id = grant["id"].as_str().unwrap();
    let (status, approved) = rt
        .post_json(
            &format!("/api/v1/cross-app/grants/{grant_id}/approve"),
            &json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{approved}");
    assert_eq!(approved["writeFieldSnapshot"], json!(["status"]));
    for action in ["create", "update", "delete"] {
        let written = if action == "delete" {
            vec![]
        } else {
            vec!["status".into()]
        };
        let authority = cross_app::authorize_cross_app_operation(
            rt.pool(),
            "consumer",
            "provider",
            "records",
            action,
            &["name".into()],
            &written,
        )
        .await
        .unwrap();
        assert_eq!(authority.action, action);
        assert_eq!(authority.write_field_snapshot, vec!["status"]);
    }
    for action in ["list", "read", "execute"] {
        assert!(
            cross_app::authorize_cross_app_operation(
                rt.pool(),
                "consumer",
                "provider",
                "records",
                action,
                &["name".into()],
                &[],
            )
            .await
            .is_err(),
            "write-only grant unexpectedly authorized {action}"
        );
    }
    assert!(
        cross_app::authorize_cross_app_read(
            rt.pool(),
            "consumer",
            "provider",
            "records",
            "create",
            &["name".into()],
        )
        .await
        .is_err(),
        "read compatibility wrapper must never authorize a mutation"
    );
    for field in ["name", "id", "secret", "missing"] {
        assert!(
            cross_app::authorize_cross_app_operation(
                rt.pool(),
                "consumer",
                "provider",
                "records",
                "update",
                &[],
                &[field.into()],
            )
            .await
            .is_err(),
            "unapproved writable field: {field}"
        );
    }
    assert!(
        cross_app::authorize_cross_app_operation(
            rt.pool(),
            "consumer",
            "provider",
            "records",
            "update",
            &["status".into()],
            &["status".into()],
        )
        .await
        .is_err(),
        "writable scope must not broaden predicate/response scope"
    );
    assert!(
        cross_app::authorize_cross_app_operation(
            rt.pool(),
            "consumer",
            "provider",
            "records",
            "delete",
            &[],
            &["status".into()],
        )
        .await
        .is_err(),
        "delete must not authorize payload writes"
    );

    // A multi-action grant still authorizes exactly one operation in each
    // transaction. SELECT is permitted inside a mutation for targeting/RETURNING.
    for bound in ["", "list", "read", "create", "update", "delete", "execute"] {
        let mut tx = rt.pool().begin().await.unwrap();
        sqlx::query("SELECT set_config('rootcx.cross_app_action', $1, true)")
            .bind(bound)
            .execute(&mut *tx)
            .await
            .unwrap();
        for required in ["read", "create", "update", "delete"] {
            let allowed: bool = sqlx::query_scalar(
                "SELECT rootcx_system.cross_app_grant_allows($1, 'provider', 'consumer', $2)",
            )
            .bind(format!("app:provider:records.{required}"))
            .bind(Uuid::parse_str(grant_id).unwrap())
            .fetch_one(&mut *tx)
            .await
            .unwrap();
            let expected = matches!(bound, "create" | "update" | "delete")
                && (required == bound || required == "read");
            assert_eq!(allowed, expected, "bound={bound:?}, required={required}");
        }
        tx.rollback().await.unwrap();
    }

    let mut tampered = cross_app::authorize_cross_app_operation(
        rt.pool(),
        "consumer",
        "provider",
        "records",
        "update",
        &[],
        &["status".into()],
    )
    .await
    .unwrap();
    tampered.write_field_snapshot.push("name".into());
    let mut tx = rt.pool().begin().await.unwrap();
    assert!(
        cross_app::lock_cross_app_read_in_tx(&mut tx, &tampered)
            .await
            .is_err(),
        "transaction recheck must compare writable scope as well as readable scope"
    );
    tx.rollback().await.unwrap();
    let (status, history) = rt
        .get_json(&format!("/api/v1/cross-app/grants/{grant_id}/audit"))
        .await;
    assert_eq!(status, StatusCode::OK, "{history}");
    let approval = history
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["operation"] == "approved")
        .unwrap();
    assert_eq!(
        approval["afterState"]["writeFieldSnapshot"],
        json!(["status"])
    );

    // Delete-only needs no writable payload scope and still gets a frozen,
    // nonsensitive mutation response scope using the usual fields default.
    rt.install("deleter", "events").await;
    let (status, delete_only) = rt
        .post_json(
            "/api/v1/cross-app/grants",
            &json!({
                "consumerApp": "deleter", "providerApp": "provider", "entity": "records",
                "actions": ["delete"]
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{delete_only}");
    assert_eq!(delete_only["writeFieldSnapshot"], json!([]));
    assert!(
        !delete_only["fieldSnapshot"]
            .as_array()
            .unwrap()
            .contains(&json!("secret"))
    );
    rt.shutdown().await;
}

#[tokio::test]
async fn enabling_a_disabled_grant_conflicts_with_its_replacement_without_changing_history() {
    let rt = harness::TestRuntime::boot().await;
    rt.install("consumer", "events").await;
    rt.install_manifest(&provider("1.0.0")).await;
    let request = grant_request("consumer");
    let (status, original) = rt.post_json("/api/v1/cross-app/grants", &request).await;
    assert_eq!(status, StatusCode::CREATED, "{original}");
    let path = format!(
        "/api/v1/cross-app/grants/{}",
        original["id"].as_str().unwrap()
    );
    let (status, approved) = rt.post_json(&format!("{path}/approve"), &json!({})).await;
    assert_eq!(status, StatusCode::OK, "{approved}");
    let (status, disabled) = rt.post_json(&format!("{path}/disable"), &json!({})).await;
    assert_eq!(status, StatusCode::OK, "{disabled}");
    let (status, history_before) = rt.get_json(&format!("{path}/audit")).await;
    assert_eq!(status, StatusCode::OK, "{history_before}");
    let (status, replacement) = rt.post_json("/api/v1/cross-app/grants", &request).await;
    assert_eq!(status, StatusCode::CREATED, "{replacement}");
    let replacement_path = format!(
        "/api/v1/cross-app/grants/{}",
        replacement["id"].as_str().unwrap(),
    );

    // Both a pending replacement and an approved one exclude reactivation.
    for approve_replacement in [false, true] {
        if approve_replacement {
            let (status, body) = rt
                .post_json(&format!("{replacement_path}/approve"), &json!({}))
                .await;
            assert_eq!(status, StatusCode::OK, "{body}");
        }
        let (status, body) = rt.post_json(&format!("{path}/enable"), &json!({})).await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "approved={approve_replacement}: {body}"
        );
        let (status, unchanged) = rt.get_json(&path).await;
        assert_eq!(status, StatusCode::OK, "{unchanged}");
        assert_eq!(unchanged, disabled);
        let (status, history_after) = rt.get_json(&format!("{path}/audit")).await;
        assert_eq!(status, StatusCode::OK, "{history_after}");
        assert_eq!(
            history_after, history_before,
            "failed enable must not append a transition"
        );
    }
    let (status, revoked) = rt
        .post_json(&format!("{replacement_path}/revoke"), &json!({}))
        .await;
    assert_eq!(status, StatusCode::OK, "{revoked}");
    let (status, enabled) = rt.post_json(&format!("{path}/enable"), &json!({})).await;
    assert_eq!(status, StatusCode::OK, "{enabled}");
    assert_eq!(enabled["status"], "active");
    assert_eq!(
        enabled["version"],
        json!(disabled["version"].as_i64().unwrap() + 1)
    );
    rt.shutdown().await;
}

#[tokio::test]
async fn governance_fixes_expiry_after_lock_wait_checks_fresh_database_time() {
    use rootcx_core::governance::{cross_app, enforcement};

    let rt = harness::TestRuntime::boot().await;
    rt.install("consumer", "events").await;
    rt.install_manifest(&provider("1.0.0")).await;
    rt.create("provider", "records", &json!({"name": "visible"}))
        .await;
    let (status, grant) = rt
        .post_json("/api/v1/cross-app/grants", &grant_request("consumer"))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{grant}");
    let id = Uuid::parse_str(grant["id"].as_str().unwrap()).unwrap();
    let (status, approved) = rt
        .post_json(
            &format!("/api/v1/cross-app/grants/{id}/approve"),
            &json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{approved}");
    let authority = cross_app::authorize_cross_app_read(
        rt.pool(),
        "consumer",
        "provider",
        "records",
        "list",
        &["name".into()],
    )
    .await
    .unwrap();
    rt.register_and_login("expiry-reader@test.local").await;
    let actor: Uuid = sqlx::query_scalar(
        "SELECT id FROM rootcx_system.users WHERE email = 'expiry-reader@test.local'",
    )
    .fetch_one(rt.pool())
    .await
    .unwrap();
    let state = enforcement::ContextState {
        user_id: Some(actor),
        is_delegated: false,
        effective_perms: vec![],
        connection_id: None,
        audit_actor_id: Some(actor),
        audit_delegator_id: None,
        public_execution: None,
    };
    let mut blocker = enforcement::begin_app_tx_with_invocation_and_cross_app(
        rt.pool(),
        "provider",
        &state,
        &enforcement::InvocationContext::default(),
        Some(actor),
        None,
        "expiry_blocker",
        enforcement::TIMEOUT_INTERACTIVE_MS,
        Some(&authority),
    )
    .await
    .unwrap();
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let visible: i64 = sqlx::query_scalar("SELECT count(*) FROM provider.records")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    assert_eq!(visible, 1);

    let pool = rt.pool().clone();
    let waiting = tokio::spawn(async move {
        match enforcement::begin_app_tx_with_invocation_and_cross_app(
            &pool,
            "provider",
            &state,
            &enforcement::InvocationContext::default(),
            Some(actor),
            None,
            "expiry_waiter",
            enforcement::TIMEOUT_INTERACTIVE_MS,
            Some(&authority),
        )
        .await
        {
            Ok(mut tx) => {
                let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM provider.records")
                    .fetch_one(&mut *tx)
                    .await
                    .unwrap();
                tx.rollback().await.unwrap();
                Ok(rows)
            }
            Err(error) => Err(error.to_string()),
        }
    });
    let waiter_pid: i32 = timeout(Duration::from_secs(5), async {
        loop {
            let pid: Option<i32> = sqlx::query_scalar(
                "SELECT pid FROM pg_locks WHERE locktype = 'advisory' AND classid = 42
                  AND NOT granted AND $1 = ANY(pg_blocking_pids(pid)) LIMIT 1",
            )
            .bind(blocker_pid)
            .fetch_optional(rt.pool())
            .await
            .unwrap();
            if let Some(pid) = pid {
                break pid;
            }
            assert!(
                !waiting.is_finished(),
                "operation must block on the existing grant lock"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    // Set the fixture's deadline only after observing the wait, so machine speed
    // cannot move transaction startup past expiry. All time comparisons use PG.
    let expires_at: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "UPDATE rootcx_system.cross_app_collection_grants
            SET expires_at = clock_timestamp() + interval '100 milliseconds'
          WHERE id = $1 RETURNING expires_at",
    )
    .bind(id)
    .fetch_one(rt.pool())
    .await
    .unwrap();
    let started_before_expiry: bool =
        sqlx::query_scalar("SELECT xact_start < $1 FROM pg_stat_activity WHERE pid = $2")
            .bind(expires_at)
            .bind(waiter_pid)
            .fetch_one(rt.pool())
            .await
            .unwrap();
    assert!(started_before_expiry);
    timeout(Duration::from_secs(5), async {
        loop {
            let expired: bool = sqlx::query_scalar("SELECT clock_timestamp() >= $1")
                .bind(expires_at)
                .fetch_one(rt.pool())
                .await
                .unwrap();
            if expired {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    // Exercise both the RLS predicate in an old transaction and the fresh
    // capability recheck in a transaction whose grant-lock acquisition waited.
    let late_rows: i64 = sqlx::query_scalar("SELECT count(*) FROM provider.records")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    blocker.commit().await.unwrap();
    let outcome = timeout(Duration::from_secs(5), waiting)
        .await
        .unwrap()
        .unwrap();
    rt.shutdown().await;
    assert!(
        late_rows == 0 && outcome.is_err(),
        "expiry must deny old-transaction RLS and post-lock authorization: rows={late_rows}, waiter={outcome:?}"
    );
}

#[tokio::test]
async fn governance_fixes_expiry_renewal_and_history_are_one_scoped_transaction() {
    let rt = harness::TestRuntime::boot().await;
    rt.install("consumer", "events").await;
    rt.install("unrelated", "events").await;
    rt.install_manifest(&provider("1.0.0")).await;
    let request = grant_request("consumer");
    let (status, old) = rt.post_json("/api/v1/cross-app/grants", &request).await;
    assert_eq!(status, StatusCode::CREATED, "{old}");
    let id = Uuid::parse_str(old["id"].as_str().unwrap()).unwrap();
    let (status, approved) = rt
        .post_json(
            &format!("/api/v1/cross-app/grants/{id}/approve"),
            &json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{approved}");
    let (status, unrelated) = rt
        .post_json("/api/v1/cross-app/grants", &grant_request("unrelated"))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{unrelated}");
    let unrelated_id = Uuid::parse_str(unrelated["id"].as_str().unwrap()).unwrap();
    sqlx::query(
        "UPDATE rootcx_system.cross_app_collection_grants SET expires_at = clock_timestamp()
          WHERE id = ANY($1)",
    )
    .bind(vec![id, unrelated_id])
    .execute(rt.pool())
    .await
    .unwrap();
    // Reject replacement history after the matching expiry transition has been
    // attempted. No GET/authorization call may sweep expired grants for this test.
    sqlx::raw_sql(
        "CREATE FUNCTION rootcx_system.reject_renewal_audit() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN
             IF NEW.operation = 'created' THEN RAISE EXCEPTION 'injected renewal audit failure'; END IF;
             RETURN NEW;
         END $$;
         CREATE TRIGGER reject_renewal_audit BEFORE INSERT ON rootcx_system.cross_app_grant_audit
         FOR EACH ROW EXECUTE FUNCTION rootcx_system.reject_renewal_audit();",
    ).execute(rt.pool()).await.unwrap();
    let (status, failed) = rt.post_json("/api/v1/cross-app/grants", &request).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{failed}");
    let state: String = sqlx::query_scalar(
        "SELECT status FROM rootcx_system.cross_app_collection_grants WHERE id = $1",
    )
    .bind(id)
    .fetch_one(rt.pool())
    .await
    .unwrap();
    assert_eq!(
        state, "active",
        "failed replacement must roll back its expiry transition"
    );
    let expired_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rootcx_system.cross_app_grant_audit WHERE grant_id = $1 AND operation = 'expired'",
    ).bind(id).fetch_one(rt.pool()).await.unwrap();
    assert_eq!(expired_events, 0);
    sqlx::query("DROP TRIGGER reject_renewal_audit ON rootcx_system.cross_app_grant_audit")
        .execute(rt.pool())
        .await
        .unwrap();
    let (status, replacement) = rt.post_json("/api/v1/cross-app/grants", &request).await;
    assert_eq!(status, StatusCode::CREATED, "{replacement}");
    assert_ne!(replacement["id"], old["id"]);
    let (state, version): (String, i64) = sqlx::query_as(
        "SELECT status, version FROM rootcx_system.cross_app_collection_grants WHERE id = $1",
    )
    .bind(id)
    .fetch_one(rt.pool())
    .await
    .unwrap();
    assert_eq!(state, "expired");
    assert_eq!(version, approved["version"].as_i64().unwrap() + 1);
    let history: Vec<(Value, Value)> = sqlx::query_as(
        "SELECT before_state, after_state FROM rootcx_system.cross_app_grant_audit
          WHERE grant_id = $1 AND operation = 'expired'",
    )
    .bind(id)
    .fetch_all(rt.pool())
    .await
    .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].0["status"], "active");
    assert_eq!(history[0].1["status"], "expired");
    let state: String = sqlx::query_scalar(
        "SELECT status FROM rootcx_system.cross_app_collection_grants WHERE id = $1",
    )
    .bind(unrelated_id)
    .fetch_one(rt.pool())
    .await
    .unwrap();
    assert_eq!(
        state, "pending",
        "creation must not sweep an unrelated relationship"
    );
    rt.shutdown().await;
}

#[tokio::test]
async fn governance_fixes_uninstall_attributes_revocation_to_the_initiating_admin() {
    let rt = harness::TestRuntime::boot().await;
    rt.install("consumer", "events").await;
    rt.install_manifest(&provider("1.0.0")).await;
    let (status, grant) = rt
        .post_json("/api/v1/cross-app/grants", &grant_request("consumer"))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{grant}");
    let id = Uuid::parse_str(grant["id"].as_str().unwrap()).unwrap();
    let token = rt.register_and_login("remover@test.local").await;
    let remover: Uuid =
        sqlx::query_scalar("SELECT id FROM rootcx_system.users WHERE email = 'remover@test.local'")
            .fetch_one(rt.pool())
            .await
            .unwrap();
    sqlx::query("INSERT INTO rootcx_system.rbac_assignments (user_id, role) VALUES ($1, 'admin')")
        .bind(remover)
        .execute(rt.pool())
        .await
        .unwrap();
    let (status, body) = rt
        .request_as(Method::DELETE, "/api/v1/apps/provider", &token, None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let actor: Option<Uuid> = sqlx::query_scalar(
        "SELECT actor_id FROM rootcx_system.cross_app_grant_audit
          WHERE grant_id = $1 AND operation = 'auto_revoked'",
    )
    .bind(id)
    .fetch_one(rt.pool())
    .await
    .unwrap();
    rt.shutdown().await;
    assert_eq!(
        actor,
        Some(remover),
        "uninstall must record its caller, not the original grant requester"
    );
}
