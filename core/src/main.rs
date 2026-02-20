mod install;

use std::path::PathBuf;
use std::sync::Arc;

use rootcx_core::{Runtime, server};
use rootcx_postgres_mgmt::{PostgresManager, data_base_dir};
use tokio::sync::Mutex;
use tracing_subscriber::EnvFilter;

const PG_PORT: u16 = 5480;
const API_PORT: u16 = 9100;

fn rootcx_home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").expect("HOME not set")).join(".rootcx")
}

/// Search candidate paths, return first match. Panics with `label` if none found.
fn resolve_resource(env_var: &str, suffix: &str, check: fn(&PathBuf) -> bool, label: &str) -> PathBuf {
    let candidates: Vec<PathBuf> = std::iter::empty()
        .chain(std::env::var(env_var).ok().map(PathBuf::from))
        .chain(std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("../resources").join(suffix))))
        .chain(std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("resources").join(suffix))))
        .chain(Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources").join(suffix)))
        .collect();

    for p in &candidates {
        if check(p) {
            return p.clone();
        }
    }
    panic!("bundled {label} not found in any search path");
}

fn resolve_pg_root() -> PathBuf {
    // PG is a directory with a bin/pg_ctl inside; scan for the first matching subdir
    let resources = resolve_resource("ROOTCX_PG_RESOURCES", "", |p| p.is_dir(), "PostgreSQL resources dir");
    std::fs::read_dir(&resources)
        .ok()
        .and_then(|rd| {
            rd.flatten().find_map(|e| {
                let p = e.path();
                (p.is_dir() && p.join("bin/pg_ctl").exists()).then_some(p)
            })
        })
        .unwrap_or_else(|| {
            // env override points directly to a pg root
            if resources.join("bin/pg_ctl").exists() { resources } else { panic!("bundled PostgreSQL not found") }
        })
}

fn resolve_bun_bin() -> PathBuf {
    resolve_resource("ROOTCX_BUN_BIN", "bun", |p| p.is_file(), "Bun")
}

fn pid_path() -> PathBuf {
    rootcx_home().join("runtime.pid")
}

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
        if daemon {
            let _ = std::fs::remove_file(pid_path());
        }
        let mut rt = rt_clone.lock().await;
        let _ = rt.shutdown().await;
        std::process::exit(0);
    });

    if let Err(e) = server::serve(runtime, API_PORT).await {
        tracing::error!("HTTP server error: {e}");
        if daemon {
            let _ = std::fs::remove_file(pid_path());
        }
        std::process::exit(1);
    }
}
