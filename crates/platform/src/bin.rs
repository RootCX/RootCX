use std::path::{Path, PathBuf};

pub const TARGET_TRIPLE: &str = env!("ROOTCX_TARGET");

pub fn binary_name(name: &str) -> String {
    if cfg!(windows) { format!("{name}.exe") } else { name.to_string() }
}

pub fn binary_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(binary_name(name))
}

pub fn bundled_binary(name: &str) -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let candidates: [Option<PathBuf>; 3] = [
        Some(dir.join(binary_name(&format!("{name}-{}", TARGET_TRIPLE)))),
        Some(dir.join(binary_name(name))),
        crate::dirs::rootcx_home().ok().map(|h| h.join("bin").join(binary_name(name))),
    ];
    candidates.into_iter().flatten().find(|p| p.exists())
}

pub fn runtime_installed() -> bool {
    crate::dirs::rootcx_home().ok()
        .map(|h| h.join("bin").join(binary_name("rootcx-core")).is_file())
        .unwrap_or(false)
}
