use super::files::{self, Files};
use crate::{api_error::ApiError, routes::SharedRuntime};
use std::{
    collections::HashSet,
    path::{Component, Path},
};

fn archive_entries(bytes: &[u8]) -> Result<Vec<(std::path::PathBuf, Vec<u8>)>, ApiError> {
    use std::io::Read;
    if bytes.len() > 50 * 1024 * 1024 {
        return Err(ApiError::BadRequest("artifact too large".into()));
    }
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
    let mut result = Vec::new();
    let mut total = 0u64;
    let mut names = HashSet::new();
    for entry in archive.entries().map_err(files::io)? {
        let mut entry = entry.map_err(files::io)?;
        let path = entry.path().map_err(files::io)?.into_owned();
        if path.as_os_str().is_empty()
            || path
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
            || !names.insert(path.clone())
        {
            return Err(ApiError::BadRequest("unsafe artifact path".into()));
        }
        if entry.header().entry_type().is_dir() {
            continue;
        }
        if !entry.header().entry_type().is_file() {
            return Err(ApiError::BadRequest(
                "artifact links and devices are forbidden".into(),
            ));
        }
        total += entry.size();
        if total > 128 * 1024 * 1024 || result.len() >= 10000 {
            return Err(ApiError::BadRequest("expanded artifact too large".into()));
        }
        let mut content = Vec::new();
        entry.read_to_end(&mut content).map_err(files::io)?;
        result.push((path, content));
    }
    Ok(result)
}
pub fn validate_archive(bytes: &[u8]) -> Result<(), ApiError> {
    if !archive_entries(bytes)?
        .iter()
        .any(|(p, _)| p == Path::new("index.html"))
    {
        return Err(ApiError::BadRequest("frontend index.html missing".into()));
    }
    Ok(())
}

pub async fn publish(
    rt: &SharedRuntime,
    app: &str,
    user: uuid::Uuid,
    manifest: &rootcx_types::AppManifest,
    sources: &Files,
    frontend: Vec<u8>,
) -> Result<(), ApiError> {
    let staged = rt
        .data_dir()
        .join("frontend-releases")
        .join(app)
        .join(uuid::Uuid::new_v4().to_string());
    tokio::fs::create_dir_all(&staged)
        .await
        .map_err(files::io)?;
    for (path, content) in archive_entries(&frontend)? {
        let target = staged.join(path);
        tokio::fs::create_dir_all(target.parent().unwrap())
            .await
            .map_err(files::io)?;
        tokio::fs::write(target, content).await.map_err(files::io)?;
    }
    let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    ));
    let mut has_backend = false;
    for (name, bytes) in files::decode(sources)? {
        if let Some(name) = name.strip_prefix("backend/") {
            has_backend = true;
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, name, bytes.as_slice())
                .map_err(files::io)?;
        }
    }
    let backend = tar
        .into_inner()
        .map_err(files::io)?
        .finish()
        .map_err(files::io)?;
    crate::manifest::install_app(
        rt.pool(),
        manifest,
        rt.extensions(),
        user,
        rt.secret_manager(),
    )
    .await?;
    if has_backend {
        let _ = crate::routes::deploy::deploy_backend_archive(rt, app, user, backend).await?;
    }
    // Immutable assets retain the old version's URLs for already-open browsers.
    // The next directory inherits missing old assets, but index.html is always new.
    let current = rt.data_dir().join("frontends").join(app);
    if current.exists() {
        retain_assets(&current, &staged)?;
    }
    let parent = current.parent().unwrap();
    tokio::fs::create_dir_all(parent).await.map_err(files::io)?;
    let link = parent.join(format!(".next-{}", uuid::Uuid::new_v4()));
    #[cfg(unix)]
    std::os::unix::fs::symlink(&staged, &link).map_err(files::io)?;
    #[cfg(not(unix))]
    return Err(ApiError::Unavailable("hosted builder requires Unix".into()));
    if current.symlink_metadata().is_ok_and(|m| m.is_dir()) {
        // One-time conversion of a legacy directory. Subsequent activations use rename.
        tokio::fs::rename(
            &current,
            parent.join(format!(".legacy-{app}-{}", uuid::Uuid::new_v4())),
        )
        .await
        .map_err(files::io)?;
    }
    tokio::fs::rename(link, current).await.map_err(files::io)?;
    Ok(())
}

fn retain_assets(old: &Path, new: &Path) -> Result<(), ApiError> {
    for entry in std::fs::read_dir(old).map_err(files::io)? {
        let entry = entry.map_err(files::io)?;
        let target = new.join(entry.file_name());
        let kind = entry.file_type().map_err(files::io)?;
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            std::fs::create_dir_all(&target).map_err(files::io)?;
            retain_assets(&entry.path(), &target)?;
        } else if kind.is_file() && !target.exists() && entry.file_name() != "index.html" {
            std::fs::copy(entry.path(), target).map_err(files::io)?;
        }
    }
    Ok(())
}
