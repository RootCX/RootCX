mod commands;
mod forge;
mod launch;
mod menu;
mod state;
mod terminal;

use state::AppState;
use tauri::{Emitter, Manager};
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
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            commands::get_os_status,
            commands::boot_runtime,
            commands::shutdown_runtime,
            commands::get_forge_status,
            commands::read_dir,
            commands::read_file,
            commands::write_file,
            commands::sync_view_menu,
            commands::read_launch_config,
            commands::init_launch_config,
            commands::spawn_terminal,
            commands::terminal_write,
            commands::terminal_resize,
        ])
        .setup(|app| {
            let view_menu = menu::setup(app)?;
            app.manage(view_menu);

            app.manage(tokio::sync::Mutex::new(terminal::TerminalState::default()));

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
        .on_menu_event(|app, event| {
            let id = event.id().as_ref();
            if app.state::<menu::ViewMenuItems>().0.contains_key(id) {
                let _ = app.emit("toggle-view", id);
            } else if id == "run" {
                let _ = app.emit("run", ());
            } else if id == "reset-layout" {
                let _ = app.emit("reset-layout", ());
            }
        })
        .build(tauri::generate_context!())
        .expect("failed to build tauri application")
        .run(|_app, event| {
            if let tauri::RunEvent::Exit = event {
                let state = _app.state::<AppState>().inner().clone();
                tauri::async_runtime::block_on(async {
                    state.shutdown().await;
                });
            }
        });
}
