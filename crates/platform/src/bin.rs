use std::path::{Path, PathBuf};

pub fn binary_name(name: &str) -> String {
    if cfg!(windows) { format!("{name}.exe") } else { name.to_string() }
}

pub fn binary_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(binary_name(name))
}

// Dev: Tauri convention `{name}-{target_triple}[.exe]`; installed: `{name}[.exe]`
pub fn bundled_binary(name: &str) -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let with_triple = dir.join(binary_name(&format!("{name}-{}", env!("ROOTCX_TARGET"))));
    if with_triple.exists() { return Some(with_triple); }
    let plain = dir.join(binary_name(name));
    plain.exists().then_some(plain)
}
