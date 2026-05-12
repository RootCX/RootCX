use axum::Json;
use axum::extract::{Path, State};
use serde::Serialize;
use serde_json::{Value as JsonValue, json};
use uuid::Uuid;

use super::{SharedRuntime, pool};
use super::crud::validate_app_id;
use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::extensions::rbac::policy::{resolve_permissions, has_permission};
use crate::webhooks;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WebhookResponse {
    id: Uuid,
    name: String,
    method: String,
    token: String,
    url: String,
    created_at: chrono::DateTime<chrono::Utc>,
}

pub async fn list_webhooks(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
) -> Result<Json<JsonValue>, ApiError> {
    validate_app_id(&app_id)?;
    let db = pool(&rt);

    let (_, perms) = resolve_permissions(&db, identity.user_id).await?;
    if !has_permission(&perms, &format!("app:{app_id}:webhook.read")) {
        return Err(ApiError::Forbidden(format!("missing app:{app_id}:webhook.read")));
    }

    let rows = webhooks::list_webhooks(&db, &app_id).await?;

    let result: Vec<WebhookResponse> = rows.into_iter().map(|r| {
        let url = format!("/api/v1/hooks/{}", r.token);
        WebhookResponse {
            id: r.id,
            name: r.name,
            method: r.method,
            token: r.token,
            url,
            created_at: r.created_at,
        }
    }).collect();

    Ok(Json(json!(result)))
}
