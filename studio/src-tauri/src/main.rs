// Prevents an additional console window on Windows in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let _ = fix_path_env::fix();
    rootcx_studio_lib::run();
}
