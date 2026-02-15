mod app_runner;
mod commands;
mod forge;
mod state;

use state::AppState;
use tauri::Manager;
use tracing::error;
use tracing_subscriber::EnvFilter;

pub fn run() {
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
            commands::boot_runtime,
            commands::shutdown_runtime,
            commands::install_app,
            commands::list_apps,
            commands::get_forge_status,
            commands::run_app,
            commands::stop_app,
            commands::app_logs,
            commands::list_running_apps,
        ])
        .setup(|app| {
            let app_state = AppState::from_tauri(app);
            let state = app_state.clone();
            app.manage(app_state);

            tauri::async_runtime::spawn(async move {
                if let Err(e) = state.boot().await {
                    error!("runtime boot failed: {e}");
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
                    state.stop_all_apps().await;
                    state.shutdown().await;
                });
            }
        });
}
