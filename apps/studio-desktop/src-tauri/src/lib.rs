mod commands;
mod state;

use state::AppState;
use tauri::Manager;
use tracing::error;
use tracing_subscriber::EnvFilter;

/// Entry point for the Tauri application.
///
/// Boot sequence:
/// 1. Initialise tracing (structured logs).
/// 2. Create the Kernel from bundled PostgreSQL paths.
/// 3. Register managed state so Tauri commands can access it.
/// 4. Trigger the async boot (initdb → start pg → bootstrap schema).
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Structured logging — respects RUST_LOG env var.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![
            commands::get_os_status,
            commands::boot_kernel,
            commands::shutdown_kernel,
            commands::install_app,
            commands::list_apps,
            commands::get_forge_status,
        ])
        .setup(|app| {
            // Create state inside setup() where the app handle is available,
            // so we can resolve bundled PostgreSQL binary/lib/share paths.
            let app_state = AppState::from_tauri(app);
            let state = app_state.clone();
            app.manage(app_state);

            // Fire the boot sequence on a background task.
            tauri::async_runtime::spawn(async move {
                if let Err(e) = state.boot().await {
                    error!("kernel boot failed: {e}");
                }
            });

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                let state = app.state::<AppState>().inner().clone();
                tauri::async_runtime::block_on(async {
                    if let Err(e) = state.shutdown().await {
                        error!("kernel shutdown failed: {e}");
                    }
                });
            }
        });
}
