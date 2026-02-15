use std::collections::HashMap;
use tauri::menu::{CheckMenuItem, MenuBuilder, SubmenuBuilder};
use tauri::{App, Wry};

const VIEWS: &[(&str, &str)] = &[
    ("explorer", "Explorer"),
    ("forge", "AI Forge"),
    ("welcome", "Welcome"),
    ("console", "Console"),
    ("output", "Output"),
];

pub struct ViewMenuItems(pub HashMap<String, CheckMenuItem<Wry>>);

pub fn setup(app: &mut App) -> tauri::Result<ViewMenuItems> {
    let app_menu = SubmenuBuilder::new(app, "RootCX Studio")
        .about(None)
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .quit()
        .build()?;

    let edit_menu = SubmenuBuilder::new(app, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;

    let mut items = HashMap::with_capacity(VIEWS.len());
    let mut view_builder = SubmenuBuilder::with_id(app, "view", "View");
    for (id, label) in VIEWS {
        let check = CheckMenuItem::with_id(app, *id, *label, true, true, None::<&str>)?;
        view_builder = view_builder.item(&check);
        items.insert(id.to_string(), check);
    }
    let view_menu = view_builder.build()?;

    let window_menu = SubmenuBuilder::new(app, "Window")
        .minimize()
        .close_window()
        .build()?;

    let menu = MenuBuilder::new(app)
        .item(&app_menu)
        .item(&edit_menu)
        .item(&view_menu)
        .item(&window_menu)
        .build()?;

    app.set_menu(menu)?;

    Ok(ViewMenuItems(items))
}
