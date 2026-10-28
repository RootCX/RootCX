use super::{Deletion, authorize, confirm, get, prepare};
use crate::{api_error::ApiError, auth::identity::Identity, routes::SharedRuntime};
use axum::{
    Json, Router,
    extract::{Path, State},
    routing::{get as get_route, post},
};
use uuid::Uuid;

pub fn routes() -> Router<SharedRuntime> {
    Router::new()
        .route("/api/v1/app-deletions", get_route(list))
        .route("/api/v1/apps/{app_id}/deletion", post(prepare_route))
        .route("/api/v1/app-deletions/{id}", get_route(inspect))
        .route("/api/v1/app-deletions/{id}/confirm", post(confirm_route))
}

async fn prepare_route(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app): Path<String>,
) -> Result<Json<Deletion>, ApiError> {
    Ok(Json(
        prepare(rt.pool(), &app, identity.user_id, None).await?,
    ))
}

async fn inspect(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(id): Path<Uuid>,
) -> Result<Json<Deletion>, ApiError> {
    authorize(rt.pool(), identity.user_id).await?;
    Ok(Json(get(rt.pool(), id).await?))
}

async fn confirm_route(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(id): Path<Uuid>,
) -> Result<Json<Deletion>, ApiError> {
    Ok(Json(confirm(rt.pool(), id, identity.user_id, None).await?))
}

pub(crate) async fn availability(
    State(rt): State<SharedRuntime>,
    Path(params): Path<std::collections::HashMap<String, String>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Result<axum::response::Response, ApiError> {
    if let Some(app) = params.get("app_id").or_else(|| params.get("app")) {
        super::ensure_available(rt.pool(), app).await?;
    }
    Ok(next.run(request).await)
}

async fn list(
    identity: Identity,
    State(rt): State<SharedRuntime>,
) -> Result<Json<Vec<Deletion>>, ApiError> {
    authorize(rt.pool(), identity.user_id).await?;
    Ok(Json(sqlx::query_as("SELECT DISTINCT ON (app_id) * FROM rootcx_system.app_deletions WHERE state <> 'awaiting_confirmation' ORDER BY app_id,created_at DESC")
        .fetch_all(rt.pool()).await?))
}
