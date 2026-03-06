use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use serde::Deserialize;
use serde_json::{Value as JsonValue, json};
use uuid::Uuid;

use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::extensions::rbac::policy::resolve_permissions;
use crate::routes::{self, SharedRuntime};

pub(crate) async fn query_installed_integrations(pool: &sqlx::PgPool) -> Result<Vec<JsonValue>, sqlx::Error> {
    let rows: Vec<(String, String, String, Option<JsonValue>)> = sqlx::query_as(
        "SELECT id, name, version, manifest FROM rootcx_system.apps
         WHERE manifest->>'type' = 'integration' AND status = 'installed' ORDER BY name",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(|(id, name, version, m)| {
        let m = m.unwrap_or(JsonValue::Null);
        json!({
            "id": id, "name": name, "version": version,
            "description": m.get("description").and_then(|v| v.as_str()).unwrap_or(""),
            "actions": m.get("actions").unwrap_or(&json!([])),
            "configSchema": m.get("configSchema"),
            "userAuth": m.get("userAuth"),
            "webhooks": m.get("webhooks").unwrap_or(&json!([])),
            "instructions": m.get("instructions"),
        })
    }).collect())
}

pub async fn list_integrations(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
) -> Result<Json<Vec<JsonValue>>, ApiError> {
    let pool = routes::pool(&rt).await?;
    Ok(Json(query_installed_integrations(&pool).await?))
}

pub async fn list_bindings(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
) -> Result<Json<Vec<JsonValue>>, ApiError> {
    let pool = routes::pool(&rt).await?;
    let rows: Vec<(String, bool, Option<String>, String)> = sqlx::query_as(
        "SELECT integration_id, enabled, webhook_token, created_at::text
         FROM rootcx_system.integration_bindings WHERE consumer_app_id = $1",
    )
    .bind(&app_id)
    .fetch_all(&pool)
    .await?;

    Ok(Json(rows.into_iter().map(|(id, enabled, token, created)| {
        json!({ "integrationId": id, "enabled": enabled, "webhookToken": token, "createdAt": created })
    }).collect()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BindRequest {
    integration_id: String,
    #[serde(default)]
    config: Option<JsonValue>,
}

pub async fn bind(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
    Json(body): Json<BindRequest>,
) -> Result<Json<JsonValue>, ApiError> {
    let (pool, secrets) = routes::pool_and_secrets(&rt).await?;

    let consumer_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM rootcx_system.apps WHERE id = $1)",
    )
    .bind(&app_id)
    .fetch_one(&pool)
    .await?;

    if !consumer_exists {
        return Err(ApiError::BadRequest(format!("app '{app_id}' not installed — sync your manifest first")));
    }

    let manifest: Option<(JsonValue,)> = sqlx::query_as(
        "SELECT manifest FROM rootcx_system.apps
         WHERE id = $1 AND manifest->>'type' = 'integration' AND status = 'installed'",
    )
    .bind(&body.integration_id)
    .fetch_optional(&pool)
    .await?;

    let manifest = manifest
        .ok_or_else(|| ApiError::NotFound(format!("integration '{}' not found", body.integration_id)))?
        .0;

    let (token,): (String,) = sqlx::query_as(
        "INSERT INTO rootcx_system.integration_bindings (consumer_app_id, integration_id, webhook_token)
         VALUES ($1, $2, $3)
         ON CONFLICT (consumer_app_id, integration_id) DO UPDATE SET enabled = true
         RETURNING webhook_token",
    )
    .bind(&app_id)
    .bind(&body.integration_id)
    .bind(Uuid::new_v4().to_string())
    .fetch_one(&pool)
    .await?;

    if let Some(config) = &body.config {
        let key = format!("_integration.{}", body.integration_id);
        secrets
            .set(&pool, &app_id, &key, &config.to_string())
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
    }

    sync_integration_permissions(&pool, &app_id, &body.integration_id, &manifest).await?;

    let wm = routes::wm(&rt).await?;
    let config = fetch_config(&pool, &secrets, &app_id, &body.integration_id).await?;
    if let Ok(result) = wm
        .rpc(
            &body.integration_id,
            Uuid::new_v4().to_string(),
            "__bind".into(),
            json!({ "config": config, "consumerAppId": app_id, "webhookToken": token }),
            None,
        )
        .await
    {
        if let Some(merge) = result.get("mergeConfig").and_then(|v| v.as_object()) {
            let mut current = match config {
                JsonValue::Object(_) => config,
                _ => json!({}),
            };
            current.as_object_mut().unwrap().extend(merge.iter().map(|(k, v)| (k.clone(), v.clone())));
            let key = format!("_integration.{}", body.integration_id);
            let _ = secrets.set(&pool, &app_id, &key, &current.to_string()).await;
        }
    }

    Ok(Json(json!({ "message": "integration bound", "webhookToken": token })))
}

#[derive(Deserialize)]
pub(crate) struct UpdateConfigRequest {
    config: JsonValue,
}

pub async fn update_config(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, integration_id)): Path<(String, String)>,
    Json(body): Json<UpdateConfigRequest>,
) -> Result<Json<JsonValue>, ApiError> {
    let (pool, secrets) = routes::pool_and_secrets(&rt).await?;

    let bound: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM rootcx_system.integration_bindings
         WHERE consumer_app_id = $1 AND integration_id = $2)",
    )
    .bind(&app_id)
    .bind(&integration_id)
    .fetch_one(&pool)
    .await?;

    if !bound {
        return Err(ApiError::NotFound("binding not found".into()));
    }

    let key = format!("_integration.{integration_id}");
    secrets
        .set(&pool, &app_id, &key, &body.config.to_string())
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(json!({ "message": "config updated" })))
}

pub async fn unbind(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, integration_id)): Path<(String, String)>,
) -> Result<Json<JsonValue>, ApiError> {
    let (pool, secrets) = routes::pool_and_secrets(&rt).await?;

    sqlx::query(
        "DELETE FROM rootcx_system.integration_bindings
         WHERE consumer_app_id = $1 AND integration_id = $2",
    )
    .bind(&app_id)
    .bind(&integration_id)
    .execute(&pool)
    .await?;

    let key = format!("_integration.{integration_id}");
    let _ = secrets.delete(&pool, &app_id, &key).await;

    // starts_with avoids LIKE wildcard injection from snake_case underscores
    sqlx::query(
        "DELETE FROM rootcx_system.rbac_permissions
         WHERE app_id = $1 AND starts_with(key, $2)",
    )
    .bind(&app_id)
    .bind(format!("integration.{integration_id}."))
    .execute(&pool)
    .await?;

    Ok(Json(json!({ "message": "integration unbound" })))
}

pub async fn execute_action(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, integration_id, action_id)): Path<(String, String, String)>,
    Json(input): Json<JsonValue>,
) -> Result<Json<JsonValue>, ApiError> {
    let (pool, secrets) = routes::pool_and_secrets(&rt).await?;
    let wm = routes::wm(&rt).await?;

    if !identity.user_id.is_nil() {
        let perm = format!("integration.{integration_id}.{action_id}");
        let (_, perms) = resolve_permissions(&pool, &app_id, identity.user_id).await?;
        if !perms.iter().any(|p| p == "*" || p == &perm) {
            return Err(ApiError::Forbidden(format!("permission denied: {perm}")));
        }
    }

    let bound: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM rootcx_system.integration_bindings
         WHERE consumer_app_id = $1 AND integration_id = $2 AND enabled = true)",
    )
    .bind(&app_id)
    .bind(&integration_id)
    .fetch_one(&pool)
    .await?;

    if !bound {
        return Err(ApiError::Forbidden(format!(
            "app '{app_id}' not bound to integration '{integration_id}'"
        )));
    }

    let config = fetch_config(&pool, &secrets, &app_id, &integration_id).await?;

    let user_credentials = if !identity.user_id.is_nil() {
        let key = format!("_iuc.{integration_id}.{}", identity.user_id);
        match secrets.get(&pool, &app_id, &key).await {
            Ok(Some(raw)) => serde_json::from_str(&raw).unwrap_or(JsonValue::Null),
            _ => JsonValue::Null,
        }
    } else {
        JsonValue::Null
    };

    let result = wm
        .rpc(
            &integration_id,
            Uuid::new_v4().to_string(),
            "__integration".into(),
            json!({ "action": action_id, "input": input, "config": config, "userCredentials": user_credentials, "consumerAppId": app_id }),
            None,
        )
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(result))
}

pub async fn webhook_ingress(
    State(rt): State<SharedRuntime>,
    Path(token): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<JsonValue>, ApiError> {
    let (pool, secrets) = routes::pool_and_secrets(&rt).await?;
    let wm = routes::wm(&rt).await?;

    let (consumer_app_id, integration_id): (String, String) = sqlx::query_as(
        "SELECT consumer_app_id, integration_id FROM rootcx_system.integration_bindings
         WHERE webhook_token = $1 AND enabled = true",
    )
    .bind(&token)
    .fetch_optional(&pool)
    .await?
    .ok_or_else(|| ApiError::NotFound("invalid webhook token".into()))?;

    let config = fetch_config(&pool, &secrets, &consumer_app_id, &integration_id).await?;

    let hdr_map: JsonValue = headers
        .iter()
        .filter_map(|(k, v)| v.to_str().ok().map(|v| (k.as_str().to_string(), json!(v))))
        .collect::<serde_json::Map<String, JsonValue>>()
        .into();

    let payload = serde_json::from_slice(&body)
        .unwrap_or_else(|_| json!(String::from_utf8_lossy(&body).into_owned()));

    use base64::Engine;
    let raw_body = base64::engine::general_purpose::STANDARD.encode(&body);

    let result = wm
        .rpc(
            &integration_id,
            Uuid::new_v4().to_string(),
            "__webhook".into(),
            json!({ "headers": hdr_map, "body": payload, "rawBody": raw_body, "config": config, "consumerAppId": consumer_app_id }),
            None,
        )
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(result))
}

pub(super) async fn fetch_config(
    pool: &sqlx::PgPool,
    secrets: &crate::secrets::SecretManager,
    consumer_app_id: &str,
    integration_id: &str,
) -> Result<JsonValue, ApiError> {
    let key = format!("_integration.{integration_id}");
    match secrets
        .get(pool, consumer_app_id, &key)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
    {
        Some(json_str) => serde_json::from_str(&json_str)
            .map_err(|e| ApiError::Internal(format!("bad config: {e}"))),
        None => Ok(JsonValue::Null),
    }
}

async fn sync_integration_permissions(
    pool: &sqlx::PgPool,
    app_id: &str,
    integration_id: &str,
    manifest: &JsonValue,
) -> Result<(), ApiError> {
    let Some(actions) = manifest.get("actions").and_then(|a| a.as_array()) else {
        return Ok(());
    };

    let (keys, descs): (Vec<String>, Vec<String>) = actions
        .iter()
        .filter_map(|a| {
            let id = a.get("id")?.as_str()?;
            let name = a.get("name").and_then(|v| v.as_str()).unwrap_or(id);
            Some((
                format!("integration.{integration_id}.{id}"),
                format!("{name} via {integration_id}"),
            ))
        })
        .unzip();

    if !keys.is_empty() {
        sqlx::query(
            "INSERT INTO rootcx_system.rbac_permissions (app_id, key, description)
             SELECT $1, unnest($2::text[]), unnest($3::text[])
             ON CONFLICT (app_id, key) DO NOTHING",
        )
        .bind(app_id)
        .bind(&keys)
        .bind(&descs)
        .execute(pool)
        .await?;
    }

    Ok(())
}
