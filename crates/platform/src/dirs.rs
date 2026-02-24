use std::path::PathBuf;
use crate::PlatformError;

pub fn home_dir() -> Result<PathBuf, PlatformError> {
    #[cfg(unix)]
    { std::env::var_os("HOME").map(PathBuf::from).ok_or(PlatformError("home directory")) }
    #[cfg(windows)]
    { std::env::var_os("USERPROFILE").map(PathBuf::from).ok_or(PlatformError("home directory")) }
}

pub fn data_dir() -> Result<PathBuf, PlatformError> {
    #[cfg(target_os = "macos")]
    { return home_dir().map(|h| h.join("Library/Application Support/RootCX")); }
    #[cfg(target_os = "linux")]
    {
        if let Some(p) = std::env::var_os("XDG_DATA_HOME") { return Ok(PathBuf::from(p).join("RootCX")); }
        return home_dir().map(|h| h.join(".local/share/RootCX"));
    }
    #[cfg(windows)]
    { std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("RootCX")).ok_or(PlatformError("APPDATA")) }
}

// macOS uses XDG-compatible path intentionally — avoids splitting config across
// ~/Library/Preferences and ~/.config; consistent behaviour across all Unices.
pub fn config_dir() -> Result<PathBuf, PlatformError> {
    #[cfg(not(windows))]
    {
        if let Some(p) = std::env::var_os("XDG_CONFIG_HOME") { return Ok(PathBuf::from(p).join("rootcx")); }
        return home_dir().map(|h| h.join(".config/rootcx"));
    }
    #[cfg(windows)]
    { std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("rootcx")).ok_or(PlatformError("APPDATA")) }
}

pub fn rootcx_home() -> Result<PathBuf, PlatformError> {
    home_dir().map(|h| h.join(".rootcx"))
}

// Resolution order: $ROOTCX_RESOURCES env > dev Cargo.toml dir > macOS bundle > exe-adjacent
pub fn resources_dir(manifest_dir: &str) -> Result<PathBuf, PlatformError> {
    if let Ok(p) = std::env::var("ROOTCX_RESOURCES") { return Ok(p.into()); }
    let dev = PathBuf::from(manifest_dir).join("resources");
    if dev.is_dir() { return Ok(dev); }
    let exe = std::env::current_exe().map_err(|_| PlatformError("executable path"))?;
    let dir = exe.parent().ok_or(PlatformError("executable parent"))?;
    #[cfg(target_os = "macos")]
    if let Some(contents) = dir.parent() {
        let p = contents.join("Resources/resources");
        if p.is_dir() { return Ok(p); }
    }
    Ok(dir.join("resources"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn home_resolves()    { home_dir().unwrap(); }
    #[test] fn data_has_rootcx()  { assert!(data_dir().unwrap().to_string_lossy().contains("RootCX")); }
    #[test] fn home_ends_rootcx() { assert!(rootcx_home().unwrap().ends_with(".rootcx")); }
}
