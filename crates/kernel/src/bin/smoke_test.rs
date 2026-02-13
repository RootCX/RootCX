/// Smoke test: boots the Kernel with bundled PG + app manifests, verifies tables, then shuts down.
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

    // Resolve paths relative to the workspace root
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();

    let tauri_resources = workspace_root
        .join("apps")
        .join("studio-desktop")
        .join("src-tauri")
        .join("resources");

    // Find bundled PG
    let pg_dir = tauri_resources.join("pg");
    let pg_root = find_pg_root(&pg_dir).expect("no PostgreSQL installation found in resources/pg/");
    let bin_dir = pg_root.join("bin");
    let lib_dir = pg_root.join("lib");

    let data_dir = data_base_dir()
        .expect("failed to resolve data directory")
        .join("data")
        .join("pg");

    let apps_dir = tauri_resources.join("apps");

    println!("PG bin   : {}", bin_dir.display());
    println!("Data dir : {}", data_dir.display());
    println!("Apps dir : {}", apps_dir.display());
    println!("Port     : {PG_PORT}\n");

    let pg = PostgresManager::new(bin_dir, data_dir, PG_PORT)
        .with_lib_dir(lib_dir);

    let mut kernel = Kernel::new(pg).with_apps_dir(apps_dir);

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

    // Verify: list schemas and tables
    if let Some(pool) = kernel.pool() {
        println!("\n--- Installed Apps ---");
        let apps: Vec<(String, String, String)> =
            sqlx::query_as("SELECT id, name, version FROM rootcx_system.apps ORDER BY name")
                .fetch_all(pool)
                .await
                .unwrap_or_default();

        for (id, name, version) in &apps {
            println!("  {id:20} {name:20} v{version}");
        }

        println!("\n--- Schemas ---");
        let schemas: Vec<(String,)> = sqlx::query_as(
            "SELECT schema_name FROM information_schema.schemata \
             WHERE schema_name NOT IN ('pg_catalog', 'information_schema', 'pg_toast', 'public') \
             ORDER BY schema_name",
        )
        .fetch_all(pool)
        .await
        .unwrap_or_default();

        for (schema,) in &schemas {
            println!("  {schema}");
        }

        println!("\n--- Tables per Schema ---");
        let tables: Vec<(String, String)> = sqlx::query_as(
            "SELECT table_schema, table_name FROM information_schema.tables \
             WHERE table_schema NOT IN ('pg_catalog', 'information_schema', 'pg_toast', 'public') \
             ORDER BY table_schema, table_name",
        )
        .fetch_all(pool)
        .await
        .unwrap_or_default();

        let mut current_schema = String::new();
        for (schema, table) in &tables {
            if *schema != current_schema {
                println!("\n  [{schema}]");
                current_schema = schema.clone();
            }
            println!("    {table}");
        }
    }

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
