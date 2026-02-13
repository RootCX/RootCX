/// Smoke test: boots the Kernel, prints status, then shuts down.
///
/// Run with:
///   cargo run -p rootcx-kernel --bin smoke_test
use rootcx_kernel::Kernel;
use rootcx_postgres_mgmt::PostgresManager;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    println!("=== RootCX Kernel Smoke Test ===\n");

    let pg = PostgresManager::with_defaults(5480).expect("failed to resolve data dir");
    println!("Data dir : {}", pg.data_dir().display());
    println!("Port     : {}\n", pg.port());

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
