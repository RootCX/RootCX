mod install;

use std::path::PathBuf;
use std::sync::Arc;

use rootcx_core::{Runtime, server};
use rootcx_postgres_mgmt::{PostgresManager, data_base_dir};
use tokio::sync::Mutex;
use tracing_subscriber::EnvFilter;

const PG_PORT: u16 = 5480;
const API_PORT: u16 = 9100;

fn resources_dir() -> PathBuf {
    rootcx_platform::dirs::resources_dir(env!("CARGO_MANIFEST_DIR"))
}

fn resolve_pg_root() -> PathBuf {
    let res = resources_dir();
    std::fs::read_dir(&res).expect("cannot read resources dir").flatten()
        .find_map(|e| {
            let p = e.path();
            (p.is_dir() && rootcx_platform::bin::binary_path(&p.join("bin"), "pg_ctl").exists()).then_some(p)
        })
        .unwrap_or_else(|| panic!("no PostgreSQL bundle in {}", res.display()))
}

fn resolve_bun_bin() -> PathBuf {
    let p = rootcx_platform::bin::binary_path(&resources_dir(), "bun");
    assert!(p.is_file(), "Bun not found at {}", p.display());
    p
}

fn rootcx_home() -> PathBuf {
    rootcx_platform::dirs::rootcx_home().expect("cannot determine home directory")
}

fn pid_path() -> PathBuf { rootcx_home().join("runtime.pid") }

#[tokio::main]
async fn main() {
    if std::env::args().nth(1).as_deref() == Some("install") {
        install::run(rootcx_home(), resolve_pg_root(), resolve_bun_bin());
        return;
    }

    let daemon = std::env::args().any(|a| a == "--daemon");

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let pg_root = resolve_pg_root();
    let bun_bin = resolve_bun_bin();
    let data_dir = data_base_dir().expect("failed to resolve data dir");

    let pg =
        PostgresManager::new(pg_root.join("bin"), data_dir.join("data/pg"), PG_PORT).with_lib_dir(pg_root.join("lib"));
    let runtime = Arc::new(Mutex::new(Runtime::new(pg, data_dir, bun_bin)));

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
