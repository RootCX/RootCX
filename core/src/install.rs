use std::path::Path;
use std::path::PathBuf;

pub fn run(home: PathBuf, pg_root: PathBuf) {
    let bin_dir = home.join("bin");
    let res_dir = home.join("resources");
    let log_dir = home.join("logs");

    for d in [&bin_dir, &res_dir, &log_dir] {
        std::fs::create_dir_all(d).unwrap_or_else(|e| {
            eprintln!("failed to create {}: {e}", d.display());
            std::process::exit(1);
        });
    }

    let self_exe = std::env::current_exe().expect("cannot determine own path");
    let target = bin_dir.join("rootcx-core");
    std::fs::copy(&self_exe, &target).expect("failed to copy binary");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755));
    }

    let pg_dest = res_dir.join(pg_root.file_name().unwrap_or("postgresql".as_ref()));
    if pg_dest.exists() {
        let _ = std::fs::remove_dir_all(&pg_dest);
    }
    copy_recursive(&pg_root, &pg_dest).expect("failed to copy PG resources");

    println!("Installed to {}", home.display());
    println!("  binary:   {}", target.display());
    println!("  postgres: {}", pg_dest.display());
    println!("  logs:     {}", log_dir.display());
}

fn copy_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)?.flatten() {
        let (s, d) = (entry.path(), dst.join(entry.file_name()));
        if s.is_dir() {
            copy_recursive(&s, &d)?;
        } else {
            std::fs::copy(&s, &d)?;
            #[cfg(unix)]
            std::fs::set_permissions(&d, std::fs::metadata(&s)?.permissions())?;
        }
    }
    Ok(())
}
