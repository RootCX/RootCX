use crate::harness;

use reqwest::{Method, StatusCode};
use serde_json::json;

#[tokio::test]
async fn human_metadata_uses_entity_permissions_instead_of_claimed_app_identity() {
    let rt = harness::TestRuntime::boot().await;
    rt.install_manifest(&json!({
        "appId": "provider", "name": "provider", "version": "1.0.0",
        "dataContract": [{
            "entityName": "records",
            "fields": [
                {"name": "name", "type": "text", "default_value": "private-default"},
                {"name": "secret", "type": "text", "sensitive": true,
                 "enum_values": ["private-constraint"]},
                {"name": "user_id", "type": "uuid", "owner": true}
            ],
            "indexes": [{"columns": ["secret"]}]
        }, {
            "entityName": "hidden",
            "fields": [{"name": "secret", "type": "text"}]
        }]
    }))
    .await;
    rt.install("unrelated", "hidden").await;
    let token = rt.create_user("metadata@test.local").await;
    let uid: uuid::Uuid = sqlx::query_scalar(
        "SELECT id FROM rootcx_system.users WHERE email = 'metadata@test.local'",
    )
    .fetch_one(rt.pool())
    .await
    .unwrap();
    sqlx::query("DELETE FROM rootcx_system.rbac_assignments WHERE user_id = $1")
        .bind(uid)
        .execute(rt.pool())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO rootcx_system.rbac_roles (name, inherits, permissions)
         VALUES ('metadata_reader', '{}', ARRAY['tool:describe_app', 'tool:list_apps'])",
    )
    .execute(rt.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO rootcx_system.rbac_assignments (user_id, role) VALUES ($1, 'metadata_reader')",
    )
    .bind(uid)
    .execute(rt.pool())
    .await
    .unwrap();

    // Use the same caller throughout. Removing a permission must take effect
    // without a new token; changing appId must never change visibility.
    for permission in [None, Some("read"), Some("read.own"), Some("read.shared"), Some("create"), None] {
        let mut permissions = vec![
            "tool:describe_app".to_string(),
            "tool:list_apps".to_string(),
        ];
        if let Some(permission) = permission {
            permissions.push(format!("app:provider:records.{permission}"));
        }
        sqlx::query(
            "UPDATE rootcx_system.rbac_roles SET permissions = $1 WHERE name = 'metadata_reader'",
        )
        .bind(&permissions)
        .execute(rt.pool())
        .await
        .unwrap();
        let readable = matches!(permission, Some("read" | "read.own" | "read.shared"));
        for claimed_app in ["provider", "unrelated", "nonexistent"] {
            let (status, body) = rt
                .request_as(
                    Method::POST,
                    "/api/v1/tools/describe_app/execute",
                    &token,
                    Some(&json!({"appId": claimed_app, "args": {"app": "provider"}})),
                )
                .await;
            if readable {
                assert_eq!(
                    status,
                    StatusCode::OK,
                    "{permission:?}/{claimed_app}: {body}"
                );
                assert_eq!(
                    body,
                    json!({
                        "app": "provider", "name": "provider",
                        "dataContract": [{
                            "entityName": "records",
                            "fields": [
                                {"name": "name", "type": "text"},
                                {"name": "user_id", "type": "uuid"}
                            ]
                        }]
                    }),
                    "{permission:?}/{claimed_app}"
                );
            } else {
                // Tool execution currently maps opaque not-found errors to 500.
                // Assert the security decision without prescribing that mapping.
                assert!(!status.is_success(), "{permission:?}/{claimed_app}: {body}");
                assert!(body.get("dataContract").is_none(), "{body}");
                for hidden in ["private-default", "private-constraint", "user_id", "secret"] {
                    assert!(!body.to_string().contains(hidden), "{body}");
                }
            }
            let (status, body) = rt
                .request_as(
                    Method::POST,
                    "/api/v1/tools/list_apps/execute",
                    &token,
                    Some(&json!({"appId": claimed_app, "args": {}})),
                )
                .await;
            assert_eq!(
                status,
                StatusCode::OK,
                "{permission:?}/{claimed_app}: {body}"
            );
            let expected = if readable {
                json!([{"id": "provider", "name": "provider", "entities": ["records"]}])
            } else {
                json!([])
            };
            assert_eq!(body, expected, "{permission:?}/{claimed_app}");
        }
    }
    rt.shutdown().await;
}
