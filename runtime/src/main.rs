use std::path::PathBuf;
use std::sync::Arc;

use rootcx_postgres_mgmt::{data_base_dir, PostgresManager};
use rootcx_runtime::{Runtime, server};
use tokio::sync::Mutex;
use tracing_subscriber::EnvFilter;

const PG_PORT: u16 = 5480;
const API_PORT: u16 = 9100;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    // Bundled PG lives next to this crate
    let pg_root = find_pg_root(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources"))
        .expect("bundled PostgreSQL not found in runtime/resources/");

    let pg = PostgresManager::new(
        pg_root.join("bin"),
        data_base_dir().expect("failed to resolve data dir").join("data").join("pg"),
        PG_PORT,
    ).with_lib_dir(pg_root.join("lib"));

    let runtime = Arc::new(Mutex::new(Runtime::new(pg)));

    {
        let mut rt = runtime.lock().await;
        if let Err(e) = rt.boot().await {
            tracing::error!("runtime boot failed: {e}");
            std::process::exit(1);
        }
    }

    let rt_clone = Arc::clone(&runtime);
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        tracing::info!("shutdown signal received");
        let mut rt = rt_clone.lock().await;
        if let Err(e) = rt.shutdown().await {
            tracing::error!("shutdown error: {e}");
        }
        std::process::exit(0);
    });

    if let Err(e) = server::serve(runtime, API_PORT).await {
        tracing::error!("HTTP server error: {e}");
        std::process::exit(1);
    }
}

fn find_pg_root(dir: &std::path::Path) -> Option<PathBuf> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() && path.join("bin").join("pg_ctl").exists() {
            return Some(path);
        }
    }
    None
}
