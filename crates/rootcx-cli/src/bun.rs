use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

const BUN_VERSION: &str = include_str!("../../../BUN_VERSION").trim_ascii();

fn bun_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("cannot determine home directory")?;
    Ok(home.join(".rootcx").join("bin"))
}

fn bun_exe() -> Result<PathBuf> {
    let name = if cfg!(windows) { "bun.exe" } else { "bun" };
    Ok(bun_dir()?.join(name))
}

fn bun_target() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Ok("bun-darwin-aarch64"),
        ("macos", "x86_64") => Ok("bun-darwin-x64"),
        ("linux", "x86_64") => Ok("bun-linux-x64"),
        ("linux", "aarch64") => Ok("bun-linux-aarch64"),
        ("windows", "x86_64") => Ok("bun-windows-x64"),
        (os, arch) => bail!("no Bun binary for {os}/{arch}"),
    }
}

/// Returns the path to a working Bun binary, downloading it if needed.
pub async fn ensure() -> Result<PathBuf> {
    let exe = bun_exe()?;
    if exe.exists() {
        return Ok(exe);
    }

    let target = bun_target()?;
    let url = format!(
        "https://github.com/oven-sh/bun/releases/download/bun-v{BUN_VERSION}/{target}.zip"
    );

    eprintln!("→ downloading bun v{BUN_VERSION} …");

    let bytes = reqwest::get(&url)
        .await.context("download bun")?
        .error_for_status().context("download bun: bad status")?
        .bytes()
        .await.context("download bun: read body")?;

    let dir = bun_dir()?;
    std::fs::create_dir_all(&dir)?;

    let bin_name = if cfg!(windows) { "bun.exe" } else { "bun" };
    let entry_prefix = format!("{target}/{bin_name}");

    let cursor = std::io::Cursor::new(&bytes);
    let mut archive = zip::ZipArchive::new(cursor).context("invalid zip")?;
    let mut entry = archive.by_name(&entry_prefix).context("bun binary not found in zip")?;
    let mut out = std::fs::File::create(&exe).context("create bun binary")?;
    std::io::copy(&mut entry, &mut out).context("extract bun")?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755))?;
    }

    eprintln!("  bun v{BUN_VERSION} installed at {}", exe.display());
    Ok(exe)
}

/// Run a command with the managed Bun binary.
pub async fn exec(bun: &Path, cwd: &Path, args: &[&str], env: &[(&str, &str)]) -> Result<()> {
    let display = format!("bun {}", args.join(" "));
    let mut cmd = tokio::process::Command::new(bun);
    cmd.args(args)
        .current_dir(cwd)
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().await.with_context(|| format!("{display}: spawn failed"))?;
    if !out.status.success() {
        bail!("{display}: exited with {}", out.status);
    }
    Ok(())
}
