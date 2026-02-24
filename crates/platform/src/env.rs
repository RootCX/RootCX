use std::path::Path;

pub const PATH_SEP: char = if cfg!(windows) { ';' } else { ':' };

pub fn prepend_path(dir: &Path) -> String {
    let mut path = dir.display().to_string();
    if let Ok(existing) = std::env::var("PATH") {
        path = format!("{path}{PATH_SEP}{existing}");
    }
    path
}

pub fn dylib_path_var() -> Option<&'static str> {
    if cfg!(target_os = "macos") { Some("DYLD_LIBRARY_PATH") }
    else if cfg!(target_os = "linux") { Some("LD_LIBRARY_PATH") }
    else { None }
}
