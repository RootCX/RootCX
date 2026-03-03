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

pub fn copy_dir(src: &Path, dst: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for e in std::fs::read_dir(src)?.flatten() {
        let (s, d) = (e.path(), dst.join(e.file_name()));
        if s.symlink_metadata()?.file_type().is_symlink() { continue; }
        if s.is_dir() { copy_dir(&s, &d)?; }
        else { std::fs::copy(&s, &d)?; let _ = copy_permissions(&s, &d); }
    }
    Ok(())
}
