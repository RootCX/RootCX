use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, post};
use tower_http::cors::CorsLayer;

use crate::routes::{self, SharedRuntime};

const MAX_UPLOAD_BYTES: usize = 50 * 1024 * 1024;

pub async fn serve(runtime: SharedRuntime, port: u16) -> Result<(), std::io::Error> {
    let mut router = Router::new()
        .route("/health", get(routes::health))
        .route("/api/v1/status", get(routes::get_status))
        .route("/api/v1/apps", get(routes::list_apps).post(routes::install_app))
        .route("/api/v1/apps/schema/verify", post(routes::verify_schema))
        .route("/api/v1/apps/{app_id}", delete(routes::uninstall_app))
        .route("/api/v1/apps/{app_id}/collections/{entity}", get(routes::list_records).post(routes::create_record))
        .route("/api/v1/apps/{app_id}/collections/{entity}/query", post(routes::query_records))
        .route(
            "/api/v1/apps/{app_id}/collections/{entity}/{id}",
            get(routes::get_record).patch(routes::update_record).delete(routes::delete_record),
        )
        .route("/api/v1/workers", get(routes::all_worker_statuses))
        .route("/api/v1/apps/{app_id}/worker/start", post(routes::start_worker))
        .route("/api/v1/apps/{app_id}/worker/stop", post(routes::stop_worker))
        .route("/api/v1/apps/{app_id}/worker/status", get(routes::worker_status))
        .route("/api/v1/apps/{app_id}/rpc", post(routes::rpc_proxy))
        .route("/api/v1/apps/{app_id}/secrets", get(routes::list_secrets).post(routes::set_secret))
        .route("/api/v1/apps/{app_id}/secrets/{key_name}", delete(routes::delete_secret))
        .route("/api/v1/config/ai", get(routes::get_ai_config).put(routes::set_ai_config))
        .route("/api/v1/config/ai/forge", get(routes::get_forge_config))
        .route("/api/v1/platform/secrets", get(routes::list_platform_secrets).post(routes::set_platform_secret))
        .route("/api/v1/platform/secrets/env", get(routes::get_platform_env))
        .route("/api/v1/platform/secrets/{key_name}", delete(routes::delete_platform_secret))
        .route("/api/v1/apps/{app_id}/jobs", get(routes::list_jobs).post(routes::enqueue_job))
        .route("/api/v1/apps/{app_id}/jobs/{job_id}", get(routes::get_job))
        .route("/api/v1/apps/{app_id}/upload", post(routes::upload_file).layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES)))
        .route(
            "/api/v1/apps/{app_id}/deploy",
            post(routes::deploy_backend).layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES)),
        )
        .route("/api/v1/db/schemas", get(routes::list_schemas))
        .route("/api/v1/db/schemas/{schema}/tables", get(routes::list_tables))
        .route("/api/v1/db/query", post(routes::execute_query))
        .route("/api/v1/tools", get(crate::tools::routes::list_tools))
        .route("/api/v1/tools/{tool_name}/execute", post(crate::tools::routes::execute_tool));

    let (auth_config, rbac_cache) = {
        let rt = runtime.lock().await;
        for ext in rt.extensions() {
            if let Some(ext_router) = ext.routes() {
                router = router.merge(ext_router);
            }
        }
        (rt.auth_config().clone(), rt.rbac_cache().clone())
    };

    let router = router
        .layer(axum::Extension(auth_config))
        .layer(axum::Extension(rbac_cache))
        .layer(CorsLayer::permissive())
        .with_state(runtime);
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    tracing::info!(port = port, "runtime HTTP server listening");
    axum::serve(listener, router).await
}
