use std::path::{Path, PathBuf};

pub fn binary_name(name: &str) -> String {
    if cfg!(windows) { format!("{name}.exe") } else { name.to_string() }
}

pub fn binary_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(binary_name(name))
}
