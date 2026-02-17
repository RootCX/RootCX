use std::sync::Arc;

use axum::routing::{delete, get};
use axum::Router;
use tokio::sync::Mutex;
use tower_http::cors::CorsLayer;

use crate::routes::{self, SharedRuntime};
use crate::Runtime;

pub fn build_router(runtime: SharedRuntime) -> Router {
    Router::new()
        .route("/health", get(routes::health))
        .route("/api/v1/status", get(routes::get_status))
        .route("/api/v1/audit", get(routes::list_audit_events))
        .route("/api/v1/apps", get(routes::list_apps).post(routes::install_app))
        .route("/api/v1/apps/{app_id}", delete(routes::uninstall_app))
        .route(
            "/api/v1/apps/{app_id}/collections/{entity}",
            get(routes::list_records).post(routes::create_record),
        )
        .route(
            "/api/v1/apps/{app_id}/collections/{entity}/{id}",
            get(routes::get_record).patch(routes::update_record).delete(routes::delete_record),
        )
        .layer(CorsLayer::permissive())
        .with_state(runtime)
}

pub async fn serve(runtime: Arc<Mutex<Runtime>>, port: u16) -> Result<(), std::io::Error> {
    let router = build_router(runtime);
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    tracing::info!(port = port, "runtime HTTP server listening");
    axum::serve(listener, router).await
}
