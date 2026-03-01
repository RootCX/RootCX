use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub fn run(app_dir: PathBuf, log: &(dyn Fn(&str) + Sync)) -> Result<PathBuf, String> {
    let app_dir = fs::canonicalize(&app_dir).map_err(|e| format!("{}: {e}", app_dir.display()))?;
    for f in ["manifest.json", "package.json", "src-tauri/Cargo.toml", "src-tauri/tauri.conf.json"] {
        if !app_dir.join(f).exists() { return Err(format!("missing {f} in {}", app_dir.display())); }
    }

    let has_backend = archive_backend(&app_dir)?;
    let pm = detect_pm(&app_dir);

    log(&format!("[bundle] installing dependencies ({pm})"));
    exec(pm, &["install"], &app_dir, log)?;
    if app_dir.join("backend/package.json").exists() {
        exec(pm, &["install"], &app_dir.join("backend"), log)?;
    }

    log(&format!("[bundle] building frontend ({pm})"));
    exec(pm, &["run", "build"], &app_dir, log)?;

    log("[bundle] cargo tauri build");
    let cfg = if has_backend {
        r#"{"build":{"beforeBuildCommand":""},"bundle":{"resources":{"resources/":"resources/"}}}"#
    } else {
        r#"{"build":{"beforeBuildCommand":""}}"#
    };
    exec("cargo", &["tauri", "build", "--config", cfg], &app_dir, log)?;

    if has_backend { let _ = fs::remove_file(app_dir.join("src-tauri/resources/backend.tar.gz")); }

    Ok(app_dir.join("src-tauri/target/release/bundle"))
}

const SKIP_DIRS: &[&str] = &["node_modules", ".git", "target", ".rootcx"];
const SKIP_EXTS: &[&str] = &[".test.ts", ".test.js", ".spec.ts", ".spec.js", ".map"];

fn archive_backend(app_dir: &Path) -> Result<bool, String> {
    let backend = app_dir.join("backend");
    if !backend.is_dir() { return Ok(false); }
    let res = app_dir.join("src-tauri/resources");
    fs::create_dir_all(&res).map_err(|e| format!("create resources dir: {e}"))?;
    let gz = flate2::write::GzEncoder::new(
        fs::File::create(res.join("backend.tar.gz")).map_err(|e| format!("create tar.gz: {e}"))?,
        flate2::Compression::default(),
    );
    let mut tar = tar::Builder::new(gz);
    tar_filtered(&mut tar, &backend, Path::new("."))?;
    tar.into_inner().map_err(|e| format!("tar finish: {e}"))?.finish().map_err(|e| format!("gz finish: {e}"))?;
    Ok(true)
}

fn tar_filtered<W: Write>(tar: &mut tar::Builder<W>, src: &Path, prefix: &Path) -> Result<(), String> {
    for entry in fs::read_dir(src).into_iter().flatten().flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with('.') { continue; }
        let disk = entry.path();

        if disk.is_dir() {
            if SKIP_DIRS.contains(&name_str.as_ref()) { continue; }
            tar_filtered(tar, &disk, &prefix.join(&name))?;
        } else if disk.is_file() {
            if SKIP_EXTS.iter().any(|ext| name_str.ends_with(ext)) { continue; }
            tar.append_path_with_name(&disk, prefix.join(&name))
                .map_err(|e| format!("tar {}: {e}", disk.display()))?;
        }
    }
    Ok(())
}

fn detect_pm(dir: &Path) -> &'static str {
    if dir.join("bun.lock").exists() || dir.join("bun.lockb").exists() { "bun" }
    else if dir.join("pnpm-lock.yaml").exists() { "pnpm" }
    else { "npm" }
}

fn exec(program: &str, args: &[&str], cwd: &Path, log: &(dyn Fn(&str) + Sync)) -> Result<(), String> {
    let mut child = Command::new(program).args(args).current_dir(cwd)
        .stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().map_err(|e| format!("{program}: {e}"))?;
    let (stdout, stderr) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    std::thread::scope(|s| {
        s.spawn(|| BufReader::new(stderr).lines().flatten().for_each(|l| log(&l)));
        BufReader::new(stdout).lines().flatten().for_each(|l| log(&l));
    });
    let status = child.wait().map_err(|e| format!("{program}: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("{program} exited {}", status.code().unwrap_or(-1))) }
}
