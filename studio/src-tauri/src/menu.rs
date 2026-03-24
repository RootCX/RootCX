use std::collections::HashMap;
use std::sync::Mutex;
use tauri::menu::{CheckMenuItem, MenuBuilder, MenuItem, Submenu, SubmenuBuilder};
use tauri::{App, AppHandle, Emitter, Manager, WebviewWindow, Wry};
use tauri_plugin_store::StoreExt;

use crate::state::{RecentProject, STORE_FILE};

fn load_recents(handle: &impl Manager<Wry>) -> Vec<RecentProject> {
    handle.store(STORE_FILE)
        .ok()
        .and_then(|s| s.get("recent_projects"))
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

const VIEWS: &[(&str, &str)] = &[
    ("explorer", "Explorer"),
    ("forge", "AI Forge"),
    ("welcome", "Welcome"),
    ("console", "Console"),
    ("output", "Output"),
    ("settings", "Settings"),
];

pub struct ViewMenuItems(pub HashMap<String, CheckMenuItem<Wry>>);
pub struct RecentMenuHandle(pub Submenu<Wry>);
pub struct FocusedWindow(pub Mutex<String>);

pub fn track_window_focus(window: &WebviewWindow) {
    let app = window.app_handle().clone();
    let label = window.label().to_string();
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::Focused(true) = event {
            if let Some(state) = app.try_state::<FocusedWindow>() {
                *state.0.lock().unwrap() = label.clone();
            }
        }
    });
}

pub fn handle_menu_event(app: &AppHandle, event: &tauri::menu::MenuEvent) {
    let id = event.id().as_ref();
    let target = app.state::<FocusedWindow>().0.lock().unwrap().clone();

    if app.state::<ViewMenuItems>().0.contains_key(id) {
        let _ = app.emit_to(&target, "toggle-view", id);
    } else if id == "run" {
        let _ = app.emit_to(&target, "run", ());
    } else if id == "deploy" {
        let _ = app.emit_to(&target, "deploy", ());
    } else if id == "apply-migrations" {
        let _ = app.emit_to(&target, "apply-migrations", ());
    } else if id == "bundle" {
        let _ = app.emit_to(&target, "bundle", ());
    } else if id == "reset-layout" {
        let _ = app.emit_to(&target, "reset-layout", ());
    } else if id == "new-window" {
        let _ = crate::commands::create_window(app.clone(), None);
    } else if id == "open-folder" {
        let _ = app.emit_to(&target, "file:open-folder", ());
    } else if id == "create-project" {
        let _ = app.emit_to(&target, "file:create-project", ());
    } else if id == "close-window" {
        if let Some(win) = app.get_webview_window(&target) {
            let _ = win.close();
        }
    } else if id == "clear-recent" {
        if let Ok(store) = app.store(STORE_FILE) {
            store.delete("recent_projects");
        }
        rebuild_recent_menu(app, &[]);
    } else if let Some(idx_str) = id.strip_prefix("recent:") {
        if let Ok(idx) = idx_str.parse::<usize>() {
            if let Some(project) = load_recents(app).get(idx) {
                let _ = app.emit_to(&target, "file:open-recent", &project.path);
            }
        }
    }
}

pub fn setup(app: &mut App) -> tauri::Result<ViewMenuItems> {
    app.manage(FocusedWindow(Mutex::new("main".to_string())));

    let app_menu = SubmenuBuilder::new(app, "RootCX Studio")
        .about(None)
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .quit()
        .build()?;

    let recents = load_recents(app);
    let recent_submenu = build_recent_submenu(app, &recents)?;

    let file_menu = SubmenuBuilder::with_id(app, "file", "File")
        .item(&MenuItem::with_id(app, "new-window", "New Window", true, Some("CmdOrCtrl+Shift+N"))?)
        .separator()
        .item(&MenuItem::with_id(app, "open-folder", "Open Folder...", true, Some("CmdOrCtrl+O"))?)
        .item(&MenuItem::with_id(app, "create-project", "Create Project...", true, None::<&str>)?)
        .separator()
        .item(&recent_submenu)
        .separator()
        .item(&MenuItem::with_id(app, "close-window", "Close Window", true, Some("CmdOrCtrl+W"))?)
        .build()?;

    app.manage(RecentMenuHandle(recent_submenu));

    let edit_menu =
        SubmenuBuilder::new(app, "Edit").undo().redo().separator().cut().copy().paste().select_all().build()?;

    let mut items = HashMap::with_capacity(VIEWS.len());
    let mut view_builder = SubmenuBuilder::with_id(app, "view", "View");
    for (id, label) in VIEWS {
        let check = CheckMenuItem::with_id(app, *id, *label, true, true, None::<&str>)?;
        view_builder = view_builder.item(&check);
        items.insert(id.to_string(), check);
    }
    let reset = MenuItem::with_id(app, "reset-layout", "Reset Default Layout", true, None::<&str>)?;
    let view_menu = view_builder.separator().item(&reset).build()?;

    let run_menu = SubmenuBuilder::with_id(app, "run-menu", "Run")
        .item(&MenuItem::with_id(app, "run", "Run", true, Some("F5"))?)
        .item(&MenuItem::with_id(app, "deploy", "Deploy to Core", true, Some("CmdOrCtrl+Shift+D"))?)
        .item(&MenuItem::with_id(app, "apply-migrations", "Apply Migrations", true, Some("CmdOrCtrl+Shift+M"))?)
        .item(&MenuItem::with_id(app, "bundle", "Bundle for Distribution", true, Some("CmdOrCtrl+Shift+B"))?)
        .build()?;

    let window_menu = SubmenuBuilder::new(app, "Window").minimize().close_window().build()?;

    let menu = MenuBuilder::new(app)
        .item(&app_menu)
        .item(&file_menu)
        .item(&edit_menu)
        .item(&view_menu)
        .item(&run_menu)
        .item(&window_menu)
        .build()?;

    app.set_menu(menu)?;
    Ok(ViewMenuItems(items))
}

fn build_recent_submenu<M: Manager<Wry>>(manager: &M, recents: &[RecentProject]) -> tauri::Result<Submenu<Wry>> {
    let mut builder = SubmenuBuilder::with_id(manager, "open-recent", "Open Recent");
    if recents.is_empty() {
        builder = builder.item(&MenuItem::with_id(manager, "no-recent", "(No Recent Projects)", false, None::<&str>)?);
    } else {
        for (i, project) in recents.iter().enumerate() {
            builder = builder.item(&MenuItem::with_id(manager, format!("recent:{i}"), &project.name, true, None::<&str>)?);
        }
        builder = builder
            .separator()
            .item(&MenuItem::with_id(manager, "clear-recent", "Clear Recent", true, None::<&str>)?);
    }
    builder.build()
}

pub fn rebuild_recent_menu(app: &AppHandle, recents: &[RecentProject]) {
    let Some(handle) = app.try_state::<RecentMenuHandle>() else { return };
    let submenu = &handle.0;

    if let Ok(items) = submenu.items() {
        for item in items {
            let _ = submenu.remove(&item);
        }
    }

    if recents.is_empty() {
        if let Ok(item) = MenuItem::with_id(app, "no-recent", "(No Recent Projects)", false, None::<&str>) {
            let _ = submenu.append(&item);
        }
    } else {
        for (i, project) in recents.iter().enumerate() {
            if let Ok(item) = MenuItem::with_id(app, format!("recent:{i}"), &project.name, true, None::<&str>) {
                let _ = submenu.append(&item);
            }
        }
        if let Ok(sep) = tauri::menu::PredefinedMenuItem::separator(app) {
            let _ = submenu.append(&sep);
        }
        if let Ok(item) = MenuItem::with_id(app, "clear-recent", "Clear Recent", true, None::<&str>) {
            let _ = submenu.append(&item);
        }
    }
}
