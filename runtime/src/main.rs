mod install;

use std::path::PathBuf;
use std::sync::Arc;

use rootcx_postgres_mgmt::{data_base_dir, PostgresManager};
use rootcx_runtime::{Runtime, server};
use tokio::sync::Mutex;
use tracing_subscriber::EnvFilter;

const PG_PORT: u16 = 5480;
const API_PORT: u16 = 9100;

fn rootcx_home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").expect("HOME not set")).join(".rootcx")
}

/// Scan `dir` for a subdirectory containing `bin/pg_ctl`.
fn find_pg_root(dir: &std::path::Path) -> Option<PathBuf> {
    std::fs::read_dir(dir).ok()?.flatten().find_map(|e| {
        let p = e.path();
        p.is_dir().then(|| p.join("bin/pg_ctl").exists().then_some(p)).flatten()
    })
}

/// Resolve PG resources: $ROOTCX_PG_RESOURCES → exe-relative → CARGO_MANIFEST_DIR.
fn resolve_pg_root() -> PathBuf {
    let candidates: Vec<PathBuf> = std::iter::empty()
        .chain(std::env::var("ROOTCX_PG_RESOURCES").ok().map(PathBuf::from))
        .chain(std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("../resources"))))
        .chain(std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("resources"))))
        .chain(Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources")))
        .collect();

    for dir in &candidates {
        if let Some(found) = find_pg_root(dir) {
            return found;
        }
    }
    panic!("bundled PostgreSQL not found in any search path");
}

fn pid_path() -> PathBuf {
    rootcx_home().join("runtime.pid")
}

#[tokio::main]
async fn main() {
    if std::env::args().nth(1).as_deref() == Some("install") {
        install::run(rootcx_home(), resolve_pg_root());
        return;
    }

    let daemon = std::env::args().any(|a| a == "--daemon");

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let pg_root = resolve_pg_root();
    let data_dir = data_base_dir().expect("failed to resolve data dir");

    let pg = PostgresManager::new(
        pg_root.join("bin"),
        data_dir.join("data/pg"),
        PG_PORT,
    ).with_lib_dir(pg_root.join("lib"));

    let runtime = Arc::new(Mutex::new(Runtime::new(pg, data_dir)));

    {
        let mut rt = runtime.lock().await;
        if let Err(e) = rt.boot(API_PORT).await {
            tracing::error!("runtime boot failed: {e}");
            std::process::exit(1);
        }
    }

    if daemon {
        let _ = std::fs::write(pid_path(), std::process::id().to_string());
        tracing::info!("daemon PID file: {}", pid_path().display());
    }

    let rt_clone = Arc::clone(&runtime);
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        tracing::info!("shutdown signal received");
        if daemon { let _ = std::fs::remove_file(pid_path()); }
        let mut rt = rt_clone.lock().await;
        let _ = rt.shutdown().await;
        std::process::exit(0);
    });

    if let Err(e) = server::serve(runtime, API_PORT).await {
        tracing::error!("HTTP server error: {e}");
        if daemon { let _ = std::fs::remove_file(pid_path()); }
        std::process::exit(1);
    }
}
