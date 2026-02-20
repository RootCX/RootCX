use std::io;
use std::path::Path;

pub fn set_executable(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)) }
    #[cfg(not(unix))]
    { let _ = path; Ok(()) }
}

pub fn set_private(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) }
    #[cfg(not(unix))]
    { let _ = path; Ok(()) }
}

pub fn copy_permissions(src: &Path, dst: &Path) -> io::Result<()> {
    #[cfg(unix)]
    { std::fs::set_permissions(dst, std::fs::metadata(src)?.permissions()) }
    #[cfg(not(unix))]
    { let _ = (src, dst); Ok(()) }
}
