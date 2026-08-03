use axum::extract::{Path, State};
use axum::http::{HeaderMap, header};
use axum::response::{IntoResponse, Response};

use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::extensions::storage::backend::{PostgresBackend, StorageBackend};
use super::{pool, SharedRuntime};

/// GET /api/v1/apps/{app_id}/icon — serve the app icon image
pub async fn get_icon(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
) -> Result<Response, ApiError> {
    let pool = pool(&rt);

    let file_id: uuid::Uuid = sqlx::query_scalar(
        "SELECT f.id \
         FROM rootcx_system.files f \
         JOIN rootcx_system.apps a ON a.icon = f.id::text AND a.id = f.app_id \
         WHERE a.id = $1",
    )
    .bind(&app_id)
    .fetch_optional(&pool)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("no icon for app '{app_id}'")))?;
    let file = PostgresBackend.open(&pool, file_id, &app_id).await?;

    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, file.content_type.parse().unwrap_or(header::HeaderValue::from_static("image/png")));
    headers.insert(header::CONTENT_LENGTH, file.size.to_string().parse().unwrap());
    headers.insert(header::CACHE_CONTROL, header::HeaderValue::from_static("public, max-age=3600, stale-while-revalidate=86400"));
    Ok((headers, file.body).into_response())
}
