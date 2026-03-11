mod browser;
mod commands;
mod forge;
mod launch;
mod menu;
mod runner;
mod scaffold;
mod state;
mod terminal;

use state::AppState;
use tauri::Manager;
use tracing::error;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_log::Builder::new().level(tauri_plugin_log::log::LevelFilter::Info).build())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            commands::get_core_url,
            commands::set_core_url,
            commands::set_auth_token,
            commands::get_os_status,
            commands::boot_runtime,
            commands::shutdown_runtime,
            commands::read_dir,
            commands::read_file,
            commands::write_file,
            commands::ensure_dir,
            commands::scaffold_project,
            commands::list_presets,
            commands::get_preset_questions,
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
            commands::create_window,
            commands::get_recent_projects,
            commands::add_to_recent,
            commands::bundle_app,
            commands::clear_recent,
            forge::forge_set_cwd,
            forge::forge_create_session,
            forge::forge_list_sessions,
            forge::forge_get_messages,
            forge::forge_send_message,
            forge::forge_abort,
            forge::forge_reply_permission,
            forge::forge_reply_question,
            forge::forge_reject_question,
            forge::forge_reload_config,
        ])
        .setup(|app| {
            let view_menu = menu::setup(app)?;
            app.manage(view_menu);
            app.manage(tokio::sync::Mutex::new(terminal::TerminalState::default()));
            app.manage(tokio::sync::Mutex::new(runner::RunnerState::default()));

            if let Some(main_window) = app.get_webview_window("main") {
                menu::track_window_focus(&main_window);
            }

            let state = AppState::from_tauri(app);
            let bg = state.clone();
            app.manage(state);

            let forge_state = forge::new_state();
            app.manage(forge_state.clone());

            tauri::async_runtime::spawn(async move {
                // Skip local daemon install when connecting to a remote core
                if !bg.is_remote() {
                    match tokio::task::spawn_blocking(rootcx_client::ensure_runtime).await {
                        Ok(Ok(rootcx_client::RuntimeStatus::Ready)) => {}
                        Ok(Ok(rootcx_client::RuntimeStatus::NotInstalled)) => match
                            tokio::task::spawn_blocking(rootcx_client::prompt_runtime_install).await {
                                Ok(Err(e)) => error!("runtime: {e}"),
                                Err(e) => error!("runtime: {e}"),
                                _ => {}
                            }
                        Ok(Err(e)) => error!("runtime: {e}"),
                        Err(e) => error!("runtime: {e}"),
                    }
                }
                if let Err(e) = bg.boot().await { error!("runtime boot failed: {e}"); }
                forge::init(&forge_state).await;
            });

            Ok(())
        })
        .on_menu_event(|app, event| {
            menu::handle_menu_event(app, &event);
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
