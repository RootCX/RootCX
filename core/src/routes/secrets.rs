use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, State};
use serde::Deserialize;
use serde_json::{Value as JsonValue, json};

use super::{SharedRuntime, pool_and_secrets};
use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::secrets::SecretManager;

#[derive(Deserialize)]
pub(crate) struct SetSecretBody {
    key: String,
    value: String,
}

pub async fn set_secret(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
    Json(body): Json<SetSecretBody>,
) -> Result<Json<JsonValue>, ApiError> {
    let (pool, sm) = pool_and_secrets(&rt).await?;
    sm.set(&pool, &app_id, &body.key, &body.value).await?;
    Ok(Json(json!({ "message": format!("secret '{}' set", body.key) })))
}

pub async fn delete_secret(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, key)): Path<(String, String)>,
) -> Result<Json<JsonValue>, ApiError> {
    let (pool, sm) = pool_and_secrets(&rt).await?;
    if sm.delete(&pool, &app_id, &key).await? {
        Ok(Json(json!({ "message": format!("secret '{key}' deleted") })))
    } else {
        Err(ApiError::NotFound(format!("secret '{key}' not found")))
    }
}

pub async fn list_secrets(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
) -> Result<Json<Vec<String>>, ApiError> {
    let (pool, _) = pool_and_secrets(&rt).await?;
    Ok(Json(SecretManager::list_keys(&pool, &app_id).await?))
}

pub async fn list_scopes(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
) -> Result<Json<Vec<String>>, ApiError> {
    let (pool, _) = pool_and_secrets(&rt).await?;
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT app_id FROM rootcx_system.secrets ORDER BY app_id"
    ).fetch_all(&pool).await?;
    Ok(Json(rows.into_iter().map(|(s,)| s).collect()))
}

const PLATFORM_SCOPE: &str = "_platform";

fn validate_key_name(key: &str) -> Result<(), ApiError> {
    if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(ApiError::BadRequest("key must be non-empty alphanumeric + underscore only".into()));
    }
    Ok(())
}

async fn reject_system_user(identity: &Identity, rt: &SharedRuntime) -> Result<(), ApiError> {
    let pool = super::pool(rt).await?;
    let is_system: bool = sqlx::query_scalar("SELECT is_system FROM rootcx_system.users WHERE id = $1")
        .bind(identity.user_id)
        .fetch_optional(&pool)
        .await?
        .unwrap_or(false);
    if is_system {
        return Err(ApiError::Forbidden("system users cannot manage platform secrets".into()));
    }
    Ok(())
}

pub async fn set_platform_secret(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Json(body): Json<SetSecretBody>,
) -> Result<Json<JsonValue>, ApiError> {
    reject_system_user(&identity, &rt).await?;
    validate_key_name(&body.key)?;
    let (pool, sm) = pool_and_secrets(&rt).await?;
    sm.set(&pool, PLATFORM_SCOPE, &body.key, &body.value).await?;
    Ok(Json(json!({ "message": format!("platform secret '{}' set", body.key) })))
}

pub async fn delete_platform_secret(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(key_name): Path<String>,
) -> Result<Json<JsonValue>, ApiError> {
    reject_system_user(&identity, &rt).await?;
    let (pool, sm) = pool_and_secrets(&rt).await?;
    if sm.delete(&pool, PLATFORM_SCOPE, &key_name).await? {
        Ok(Json(json!({ "message": format!("platform secret '{key_name}' deleted") })))
    } else {
        Err(ApiError::NotFound(format!("platform secret '{key_name}' not found")))
    }
}

pub async fn list_platform_secrets(
    identity: Identity,
    State(rt): State<SharedRuntime>,
) -> Result<Json<Vec<String>>, ApiError> {
    reject_system_user(&identity, &rt).await?;
    let (pool, _) = pool_and_secrets(&rt).await?;
    Ok(Json(SecretManager::list_keys(&pool, PLATFORM_SCOPE).await?))
}

pub async fn get_platform_env(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
) -> Result<Json<HashMap<String, String>>, ApiError> {
    let (pool, sm) = pool_and_secrets(&rt).await?;
    let env: HashMap<String, String> = sm.get_all_for_app(&pool, PLATFORM_SCOPE).await?.into_iter().collect();
    Ok(Json(env))
}
