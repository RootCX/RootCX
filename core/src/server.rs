use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, post};
use tower_http::cors::{AllowOrigin, CorsLayer};

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
        .route("/api/v1/federated/{identity_kind}/query", post(routes::federated_query))
        .route("/api/v1/apps/{app_id}/collections/{entity}/bulk", post(routes::bulk_create_records))
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
        .route("/api/v1/llm-models", get(routes::llm_models::list_llm_models).post(routes::llm_models::create_llm_model))
        .route("/api/v1/llm-models/{id}", axum::routing::put(routes::llm_models::update_llm_model).delete(routes::llm_models::delete_llm_model))
        .route("/api/v1/llm-models/{id}/default", axum::routing::put(routes::llm_models::set_default_llm_model))
        .route("/api/v1/config/ai/forge", get(routes::llm_models::get_forge_model))
        .route("/api/v1/platform/secrets", get(routes::list_platform_secrets).post(routes::set_platform_secret))
        .route("/api/v1/platform/secrets/env", get(routes::get_platform_env))
        .route("/api/v1/platform/secrets/{key_name}", delete(routes::delete_platform_secret))
        .route("/api/v1/apps/{app_id}/jobs", get(routes::list_jobs).post(routes::enqueue_job))
        .route("/api/v1/apps/{app_id}/upload", post(routes::upload_file).layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES)))
        .route(
            "/api/v1/apps/{app_id}/deploy",
            post(routes::deploy_backend).layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES)),
        )
        .route(
            "/api/v1/apps/{app_id}/frontend",
            post(routes::deploy_frontend).layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES)),
        )
        .route("/apps/{app_id}", get(routes::serve_frontend_root))
        .route("/apps/{app_id}/", get(routes::serve_frontend_root))
        .route("/apps/{app_id}/{*path}", get(routes::serve_frontend))
        .route("/api/v1/db/schemas", get(routes::list_schemas))
        .route("/api/v1/db/schemas/{schema}/tables", get(routes::list_tables))
        .route("/api/v1/db/query", post(routes::execute_query))
        .route("/api/v1/tools", get(crate::tools::routes::list_tools))
        .route("/api/v1/tools/{tool_name}/execute", post(crate::tools::routes::execute_tool));

    for ext in runtime.extensions() {
        if let Some(ext_router) = ext.routes() {
            router = router.merge(ext_router);
        }
    }
    let auth_config = runtime.auth_config().clone();

    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(|origin, _| {
            let o = origin.as_bytes();
            o.starts_with(b"http://localhost:") || o.starts_with(b"http://127.0.0.1:")
                || o.starts_with(b"tauri://") || o == b"http://tauri.localhost"
        }))
        .allow_methods(tower_http::cors::Any)
        .allow_headers(tower_http::cors::Any);

    let router = router
        .layer(axum::Extension(auth_config))
        .layer(cors)
        .with_state(runtime);
    let bind = if std::env::var("ROOTCX_BIND").is_ok() { "0.0.0.0" } else { "127.0.0.1" };
    let listener = tokio::net::TcpListener::bind(format!("{bind}:{port}")).await?;
    tracing::info!(port = port, bind = bind, "runtime HTTP server listening");
    axum::serve(listener, router).await
}
