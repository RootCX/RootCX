use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use serde::Deserialize;
use serde_json::{Value as JsonValue, json};
use uuid::Uuid;

use base64::Engine;

use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::extensions::rbac::policy::resolve_permissions;
use crate::routes::{self, SharedRuntime};

const PLATFORM_SCOPE: &str = "_platform";

pub(super) fn platform_secret_map(manifest: &JsonValue) -> Vec<(String, String)> {
    let Some(props) = manifest.pointer("/configSchema/properties").and_then(|v| v.as_object()) else {
        return vec![];
    };
    props.iter().filter_map(|(field, def)| {
        def.get("platformSecret").and_then(|v| v.as_str()).map(|key| (field.clone(), key.to_string()))
    }).collect()
}

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
    let (pool, secrets) = routes::pool_and_secrets(&rt).await?;
    let mut items = query_installed_integrations(&pool).await?;
    for item in &mut items {
        let map = platform_secret_map(item);
        let configured = is_configured(&pool, &secrets, item, &map).await?;
        item.as_object_mut().unwrap().insert("configured".into(), json!(configured));
    }
    Ok(Json(items))
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

    sync_integration_permissions(&pool, &app_id, &body.integration_id, &manifest).await?;

    let wm = routes::wm(&rt).await?;
    let secret_map = platform_secret_map(&manifest);
    let config = fetch_config(&pool, &secrets, &secret_map).await?;
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
            let merged = json!(merge);
            save_config_secrets(&pool, &secrets, &secret_map, &merged).await?;
        }
    }

    Ok(Json(json!({ "message": "integration bound", "webhookToken": token })))
}

pub async fn save_platform_config(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(integration_id): Path<String>,
    Json(config): Json<JsonValue>,
) -> Result<Json<JsonValue>, ApiError> {
    let (pool, secrets) = routes::pool_and_secrets(&rt).await?;
    let manifest = get_integration_manifest(&pool, &integration_id).await?;
    save_config_secrets(&pool, &secrets, &platform_secret_map(&manifest), &config).await?;
    Ok(Json(json!({ "message": "config saved" })))
}

pub async fn unbind(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, integration_id)): Path<(String, String)>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt).await?;

    sqlx::query(
        "DELETE FROM rootcx_system.integration_bindings
         WHERE consumer_app_id = $1 AND integration_id = $2",
    )
    .bind(&app_id)
    .bind(&integration_id)
    .execute(&pool)
    .await?;

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

    let perm = format!("integration.{integration_id}.{action_id}");
    let (_, perms) = resolve_permissions(&pool, &app_id, identity.user_id).await?;
    if !perms.iter().any(|p| p == "*" || p == &perm) {
        return Err(ApiError::Forbidden(format!("permission denied: {perm}")));
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

    let config = resolve_config(&pool, &secrets, &integration_id).await?;

    let key = format!("_iuc.{integration_id}.{}", identity.user_id);
    let user_credentials = match secrets.get(&pool, &app_id, &key).await {
        Ok(Some(raw)) => serde_json::from_str(&raw).unwrap_or(JsonValue::Null),
        _ => JsonValue::Null,
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

    let config = resolve_config(&pool, &secrets, &integration_id).await?;

    let hdr_map: JsonValue = headers
        .iter()
        .filter_map(|(k, v)| v.to_str().ok().map(|v| (k.as_str().to_string(), json!(v))))
        .collect::<serde_json::Map<String, JsonValue>>()
        .into();

    let payload = serde_json::from_slice(&body)
        .unwrap_or_else(|_| json!(String::from_utf8_lossy(&body).into_owned()));

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

pub(super) async fn get_integration_manifest(pool: &sqlx::PgPool, integration_id: &str) -> Result<JsonValue, ApiError> {
    let row: Option<(JsonValue,)> = sqlx::query_as(
        "SELECT manifest FROM rootcx_system.apps WHERE id = $1 AND manifest->>'type' = 'integration'",
    )
    .bind(integration_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(m,)| m).unwrap_or(JsonValue::Null))
}

async fn save_config_secrets(
    pool: &sqlx::PgPool,
    secrets: &crate::secrets::SecretManager,
    secret_map: &[(String, String)],
    config: &JsonValue,
) -> Result<(), ApiError> {
    for (field, secret_key) in secret_map {
        if let Some(val) = config.get(field).and_then(|v| v.as_str()) {
            secrets.set(pool, PLATFORM_SCOPE, secret_key, val)
                .await
                .map_err(|e| ApiError::Internal(e.to_string()))?;
        }
    }
    Ok(())
}

async fn is_configured(
    pool: &sqlx::PgPool,
    secrets: &crate::secrets::SecretManager,
    manifest: &JsonValue,
    secret_map: &[(String, String)],
) -> Result<bool, ApiError> {
    if secret_map.is_empty() { return Ok(true); }
    let required: Vec<&str> = manifest.pointer("/configSchema/required")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    for (field, key) in secret_map {
        if required.contains(&field.as_str()) {
            if secrets.get(pool, PLATFORM_SCOPE, key).await
                .map_err(|e| ApiError::Internal(e.to_string()))?.is_none() {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

pub(super) async fn resolve_config(
    pool: &sqlx::PgPool,
    secrets: &crate::secrets::SecretManager,
    integration_id: &str,
) -> Result<JsonValue, ApiError> {
    let manifest = get_integration_manifest(pool, integration_id).await?;
    fetch_config(pool, secrets, &platform_secret_map(&manifest)).await
}

async fn fetch_config(
    pool: &sqlx::PgPool,
    secrets: &crate::secrets::SecretManager,
    secret_map: &[(String, String)],
) -> Result<JsonValue, ApiError> {
    let mut config = serde_json::Map::new();
    for (field, secret_key) in secret_map {
        if let Some(val) = secrets.get(pool, PLATFORM_SCOPE, secret_key)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?
        {
            config.insert(field.clone(), JsonValue::String(val));
        }
    }
    if config.is_empty() { Ok(JsonValue::Null) } else { Ok(JsonValue::Object(config)) }
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
