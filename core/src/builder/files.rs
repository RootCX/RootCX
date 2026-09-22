use crate::api_error::ApiError;
use base64::{Engine, engine::general_purpose::STANDARD};
use std::{
    collections::BTreeMap,
    path::{Component, Path},
    time::Duration,
};
use tokio::process::Command;

pub type Files = BTreeMap<String, String>;
pub const MAX_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_FILES: usize = 4000;

pub fn app_id(id: &str) -> Result<(), ApiError> {
    if id == "core"
        || id == "assistant"
        || id.is_empty()
        || id.len() > 50
        || !id.as_bytes()[0].is_ascii_lowercase()
        || !id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
    {
        return Err(ApiError::BadRequest("invalid application ID".into()));
    }
    Ok(())
}

pub fn path(name: &str) -> Result<(), ApiError> {
    if name.is_empty()
        || name.len() > 500
        || name.contains(['\\', '\0', ':'])
        || name.split('/').any(|s| {
            s.is_empty()
                || matches!(
                    s,
                    "." | ".."
                        | ".git"
                        | ".gitattributes"
                        | ".aws"
                        | ".ssh"
                        | ".kube"
                        | ".rootcx"
                        | "node_modules"
                        | "dist"
                        | ".npmrc"
                        | ".yarnrc"
                        | ".yarnrc.yml"
                )
                || (s.starts_with(".env") && s != ".env.example")
        })
        || Path::new(name)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(ApiError::BadRequest(format!(
            "disallowed source path: {name}"
        )));
    }
    Ok(())
}

pub fn decode(files: &Files) -> Result<BTreeMap<String, Vec<u8>>, ApiError> {
    if files.is_empty() || files.len() > MAX_FILES {
        return Err(ApiError::BadRequest(
            "source file count out of bounds".into(),
        ));
    }
    let mut total = 0;
    files
        .iter()
        .map(|(name, encoded)| {
            path(name)?;
            let bytes = STANDARD
                .decode(encoded)
                .map_err(|_| ApiError::BadRequest("invalid base64 source".into()))?;
            total += bytes.len();
            if total > MAX_BYTES {
                return Err(ApiError::BadRequest("sources exceed 64 MiB".into()));
            }
            Ok((name.clone(), bytes))
        })
        .collect()
}

pub fn manifest(files: &Files, id: &str) -> Result<rootcx_types::AppManifest, ApiError> {
    let decoded = decode(files)?;
    let raw = decoded
        .get("manifest.json")
        .ok_or_else(|| ApiError::BadRequest("manifest.json required".into()))?;
    let manifest: rootcx_types::AppManifest =
        serde_json::from_slice(raw).map_err(|e| ApiError::BadRequest(e.to_string()))?;
    if manifest.app_id != id {
        return Err(ApiError::BadRequest(
            "source manifest application mismatch".into(),
        ));
    }
    crate::manifest::validate_manifest(&manifest)?;
    Ok(manifest)
}

pub async fn write(root: &Path, files: &Files) -> Result<(), ApiError> {
    let decoded = decode(files)?;
    for (name, bytes) in decoded {
        let target = root.join(name);
        tokio::fs::create_dir_all(target.parent().unwrap())
            .await
            .map_err(io)?;
        tokio::fs::write(target, bytes).await.map_err(io)?;
    }
    Ok(())
}

// Only trusted Core calls Git; neither model-authored hooks nor host Git config run.
pub async fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>, ApiError> {
    let mut cmd = Command::new("git");
    cmd.env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", "Shappy")
        .env("GIT_AUTHOR_EMAIL", "shappy@rootcx.local")
        .env("GIT_COMMITTER_NAME", "Shappy")
        .env("GIT_COMMITTER_EMAIL", "shappy@rootcx.local")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgSign=false",
            "-c",
            "core.autocrlf=false",
        ])
        .args(args)
        .current_dir(root)
        .kill_on_drop(true);
    let out = tokio::time::timeout(Duration::from_secs(60), cmd.output())
        .await
        .map_err(|_| ApiError::Unavailable("source operation timed out".into()))?
        .map_err(io)?;
    if !out.status.success() {
        tracing::error!(error = %String::from_utf8_lossy(&out.stderr), "source Git operation failed");
        return Err(ApiError::Internal("source version operation failed".into()));
    }
    Ok(out.stdout)
}

pub async fn commit(root: &Path, message: &str) -> Result<String, ApiError> {
    git(root, &["add", "--all", "--force", "--", "."]).await?;
    git(root, &["commit", "--allow-empty", "-m", message]).await?;
    Ok(String::from_utf8(git(root, &["rev-parse", "HEAD"]).await?)
        .unwrap()
        .trim()
        .into())
}

pub async fn snapshot(root: &Path, revision: &str) -> Result<Files, ApiError> {
    let bytes = git(root, &["archive", "--format=tar", revision]).await?;
    let mut result = Files::new();
    let mut archive = tar::Archive::new(bytes.as_slice());
    for entry in archive.entries().map_err(io)? {
        let mut entry = entry.map_err(io)?;
        if entry.header().entry_type().is_dir()
            || entry.header().entry_type().is_pax_global_extensions()
        {
            continue;
        }
        if !entry.header().entry_type().is_file() {
            return Err(ApiError::BadRequest("source links are forbidden".into()));
        }
        let name = entry.path().map_err(io)?.to_string_lossy().into_owned();
        path(&name)?;
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut bytes).map_err(io)?;
        result.insert(name, STANDARD.encode(bytes));
    }
    decode(&result)?;
    Ok(result)
}

pub fn io(e: std::io::Error) -> ApiError {
    ApiError::Internal(format!("source storage: {e}"))
}
