/// Smoke test: boots the Kernel, prints status, then shuts down.
///
/// Run with:
///   cargo run -p rootcx-kernel --bin smoke_test
use std::path::PathBuf;

use rootcx_kernel::Kernel;
use rootcx_postgres_mgmt::{data_base_dir, PostgresManager};
use tracing_subscriber::EnvFilter;

const PG_PORT: u16 = 5480;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    println!("=== RootCX Kernel Smoke Test ===\n");

    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();

    let pg_dir = workspace_root
        .join("apps")
        .join("studio-desktop")
        .join("src-tauri")
        .join("resources")
        .join("pg");

    let pg_root = find_pg_root(&pg_dir).expect("no PostgreSQL installation found in resources/pg/");
    let bin_dir = pg_root.join("bin");
    let lib_dir = pg_root.join("lib");

    let data_dir = data_base_dir()
        .expect("failed to resolve data directory")
        .join("data")
        .join("pg");

    println!("PG bin   : {}", bin_dir.display());
    println!("Data dir : {}", data_dir.display());
    println!("Port     : {PG_PORT}\n");

    let pg = PostgresManager::new(bin_dir, data_dir, PG_PORT)
        .with_lib_dir(lib_dir);

    let mut kernel = Kernel::new(pg);

    println!("--- Booting ---");
    match kernel.boot().await {
        Ok(()) => println!("--- Boot OK ---\n"),
        Err(e) => {
            eprintln!("Boot FAILED: {e}");
            std::process::exit(1);
        }
    }

    let status = kernel.status().await;
    println!(
        "Kernel   : {} (v{})",
        status.kernel.state, status.kernel.version
    );
    println!(
        "Postgres : {} (port {:?})",
        status.postgres.state,
        status.postgres.port
    );

    println!("\n--- Shutting down ---");
    if let Err(e) = kernel.shutdown().await {
        eprintln!("Shutdown error: {e}");
    }
    println!("--- Done ---");
}

fn find_pg_root(pg_dir: &std::path::Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(pg_dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && path.join("bin").join("postgres").exists() {
            return Some(path);
        }
    }
    None
}
