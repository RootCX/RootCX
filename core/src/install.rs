use std::path::{Path, PathBuf};

pub fn run(home: PathBuf, pg_root: PathBuf, bun_bin: PathBuf) {
    let bin_dir = home.join("bin");
    let res_dir = home.join("resources");
    let log_dir = home.join("logs");

    for d in [&bin_dir, &res_dir, &log_dir] {
        std::fs::create_dir_all(d).unwrap_or_else(|e| { eprintln!("mkdir {}: {e}", d.display()); std::process::exit(1); });
    }

    let self_exe = std::env::current_exe().expect("cannot determine own path");
    let target = rootcx_platform::bin::binary_path(&bin_dir, "rootcx-core");
    std::fs::copy(&self_exe, &target).expect("copy binary");
    let _ = rootcx_platform::fs::set_executable(&target);

    let pg_dest = res_dir.join(pg_root.file_name().unwrap_or("postgresql".as_ref()));
    if pg_dest.exists() { let _ = std::fs::remove_dir_all(&pg_dest); }
    copy_recursive(&pg_root, &pg_dest).expect("copy PG resources");

    let bun_dest = rootcx_platform::bin::binary_path(&res_dir, "bun");
    std::fs::copy(&bun_bin, &bun_dest).expect("copy Bun binary");
    let _ = rootcx_platform::fs::set_executable(&bun_dest);

    println!("Installed to {}", home.display());
}

fn copy_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)?.flatten() {
        let (s, d) = (entry.path(), dst.join(entry.file_name()));
        if s.is_dir() { copy_recursive(&s, &d)?; }
        else { std::fs::copy(&s, &d)?; let _ = rootcx_platform::fs::copy_permissions(&s, &d); }
    }
    Ok(())
}
