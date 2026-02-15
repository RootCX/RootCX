use std::sync::Arc;

use axum::routing::{delete, get, patch, post};
use axum::Router;
use tokio::sync::Mutex;
use tower_http::cors::CorsLayer;

use crate::routes::{self, SharedRuntime};
use crate::Runtime;

/// Build the axum Router with all API routes.
pub fn build_router(runtime: SharedRuntime) -> Router {
    let api = Router::new()
        // Status
        .route("/api/v1/status", get(routes::get_status))
        // Apps management
        .route("/api/v1/apps", post(routes::install_app))
        .route("/api/v1/apps", get(routes::list_apps))
        .route("/api/v1/apps/{app_id}", delete(routes::uninstall_app))
        // Collections CRUD
        .route(
            "/api/v1/apps/{app_id}/collections/{entity}",
            get(routes::list_records),
        )
        .route(
            "/api/v1/apps/{app_id}/collections/{entity}",
            post(routes::create_record),
        )
        .route(
            "/api/v1/apps/{app_id}/collections/{entity}/{id}",
            get(routes::get_record),
        )
        .route(
            "/api/v1/apps/{app_id}/collections/{entity}/{id}",
            patch(routes::update_record),
        )
        .route(
            "/api/v1/apps/{app_id}/collections/{entity}/{id}",
            delete(routes::delete_record),
        )
        .with_state(runtime);

    Router::new()
        .route("/health", get(routes::health))
        .merge(api)
        .layer(CorsLayer::permissive())
}

/// Start serving the runtime HTTP API on the given port.
pub async fn serve(runtime: Arc<Mutex<Runtime>>, port: u16) -> Result<(), std::io::Error> {
    let router = build_router(runtime);
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    tracing::info!(port = port, "runtime HTTP server listening");
    axum::serve(listener, router).await
}
