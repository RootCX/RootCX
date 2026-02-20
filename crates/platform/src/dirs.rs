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
        if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
            return Ok(PathBuf::from(xdg).join("RootCX"));
        }
        return home_dir().map(|h| h.join(".local/share/RootCX"));
    }
    #[cfg(target_os = "windows")]
    { std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("RootCX")).ok_or(PlatformError("data directory")) }
}

pub fn config_dir() -> Result<PathBuf, PlatformError> {
    #[cfg(target_os = "macos")]
    { return home_dir().map(|h| h.join(".config/rootcx")); }
    #[cfg(target_os = "linux")]
    {
        if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
            return Ok(PathBuf::from(xdg).join("rootcx"));
        }
        return home_dir().map(|h| h.join(".config/rootcx"));
    }
    #[cfg(target_os = "windows")]
    { std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("rootcx")).ok_or(PlatformError("config directory")) }
}

pub fn rootcx_home() -> Result<PathBuf, PlatformError> {
    home_dir().map(|h| h.join(".rootcx"))
}

/// Caller passes `env!("CARGO_MANIFEST_DIR")` — compile-time path per crate.
pub fn resources_dir(compile_time_manifest_dir: &str) -> PathBuf {
    if let Ok(p) = std::env::var("ROOTCX_RESOURCES") {
        return PathBuf::from(p);
    }
    let dev = PathBuf::from(compile_time_manifest_dir).join("resources");
    if dev.is_dir() { return dev; }
    std::env::current_exe()
        .expect("cannot determine exe path")
        .parent().expect("exe has no parent")
        .join("resources")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_dir_resolves() { assert!(home_dir().is_ok()); }

    #[test]
    fn data_dir_resolves() { assert!(data_dir().unwrap().to_string_lossy().contains("RootCX")); }

    #[test]
    fn rootcx_home_resolves() { assert!(rootcx_home().unwrap().ends_with(".rootcx")); }
}
