mod commands;
mod forge;
mod launch;
mod menu;
mod runner;
mod scaffold;
mod state;
mod terminal;

use state::AppState;
use tauri::{Emitter, Manager};
use tracing::error;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_log::Builder::new().level(tauri_plugin_log::log::LevelFilter::Info).build())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            commands::get_os_status,
            commands::boot_runtime,
            commands::shutdown_runtime,
            commands::get_forge_status,
            commands::start_forge,
            commands::save_forge_config,
            commands::read_dir,
            commands::read_file,
            commands::write_file,
            commands::ensure_dir,
            commands::scaffold_project,
            commands::list_presets,
            commands::get_preset_questions,
            commands::verify_schema,
            commands::sync_manifest,
            commands::deploy_backend,
            commands::run_app,
            commands::stop_deployed_worker,
            commands::resolve_instructions,
            commands::sync_view_menu,
            commands::read_launch_config,
            commands::init_launch_config,
            commands::spawn_terminal,
            commands::terminal_write,
            commands::terminal_resize,
            commands::list_platform_secrets,
            commands::set_platform_secret,
            commands::delete_platform_secret,
        ])
        .setup(|app| {
            let view_menu = menu::setup(app)?;
            app.manage(view_menu);
            app.manage(tokio::sync::Mutex::new(terminal::TerminalState::default()));
            app.manage(tokio::sync::Mutex::new(runner::RunnerState::default()));

            let state = AppState::from_tauri(app);
            let bg = state.clone();
            app.manage(state);

            tauri::async_runtime::spawn(async move {
                if let Err(e) = tokio::task::spawn_blocking(rootcx_runtime::ensure_runtime)
                    .await
                    .unwrap_or_else(|e| Err(rootcx_runtime::ClientError::RuntimeStart(e.to_string())))
                {
                    error!("failed to start core daemon: {e}");
                }
                if let Err(e) = bg.boot().await {
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
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                let state = app.state::<AppState>().inner().clone();
                let runner = app.state::<tokio::sync::Mutex<runner::RunnerState>>();
                tauri::async_runtime::block_on(async {
                    runner.lock().await.stop();
                    state.shutdown().await;
                });
            }
        });
}
