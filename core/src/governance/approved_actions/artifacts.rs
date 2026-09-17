use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::RuntimeError;
pub(crate) fn artifact_dir(data_dir: &Path, revision: Uuid) -> PathBuf {
    data_dir.join("approved-backends").join(revision.to_string())
}

pub(crate) async fn verify_artifact(
    directory: &Path, expected_digest: &str,
) -> Result<(), RuntimeError> {
    if fingerprint(directory).await? != expected_digest {
        return Err(RuntimeError::Worker("approved backend artifact digest changed".into()));
    }
    Ok(())
}

pub(crate) async fn begin_deployment(
    pool: &PgPool, app: &str, actor: Uuid,
) -> Result<Uuid, RuntimeError> {
    let revision = Uuid::new_v4();
    let mut tx = pool.begin().await.map_err(RuntimeError::Schema)?;
    sqlx::query(
        "INSERT INTO rootcx_system.backend_releases(app_id,revision) VALUES ($1,$2)
         ON CONFLICT(app_id) DO UPDATE SET revision=$2,digest=NULL",
    ).bind(app).bind(revision).execute(&mut *tx).await.map_err(RuntimeError::Schema)?;
    super::revoke_app(&mut tx, app, Some(actor), "backend replaced").await.map_err(RuntimeError::Schema)?;
    tx.commit().await.map_err(RuntimeError::Schema)?;
    Ok(revision)
}

pub(crate) async fn finish_deployment(
    pool: &PgPool, app: &str, revision: Uuid, directory: &Path,
) -> Result<(), RuntimeError> {
    let digest = fingerprint(directory).await?;
    sqlx::query("UPDATE rootcx_system.backend_releases SET digest=$3 WHERE app_id=$1 AND revision=$2")
        .bind(app).bind(revision).bind(digest).execute(pool).await.map_err(RuntimeError::Schema)?;
    Ok(())
}

pub(super) async fn fingerprint(directory: &Path) -> Result<String, RuntimeError> {
    let directory = directory.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<_, std::io::Error> {
        let root = std::fs::canonicalize(directory)?;
        let mut hash = Sha256::new();
        visit(&root, &root, Path::new(""), None, &mut hash, &mut Vec::new())?;
        Ok(hex::encode(hash.finalize()))
    }).await.map_err(|e| RuntimeError::Worker(e.to_string()))?.map_err(|e| RuntimeError::Worker(e.to_string()))
}

/// Snapshot resolved dependencies too. Links are copied as their contents only
/// when they stay inside the artifact; external imports are never sealed in.
pub(super) async fn seal(
    data_dir: &Path, app: &str, revision: Uuid, expected_digest: &str,
) -> Result<(), RuntimeError> {
    let root = data_dir.join("apps").join(app);
    let parent = data_dir.join("approved-backends");
    let destination = parent.join(revision.to_string());
    if destination.exists() {
        if fingerprint(&destination).await? != expected_digest {
            return Err(RuntimeError::Invalid("approved backend artifact changed on disk".into()));
        }
        return Ok(());
    }
    let expected = expected_digest.to_owned();
    tokio::task::spawn_blocking(move || -> Result<_, std::io::Error> {
        std::fs::create_dir_all(&parent)?;
        let staging = parent.join(format!(".staging-{}", Uuid::new_v4()));
        std::fs::create_dir(&staging)?;
        let result = (|| {
            let root = std::fs::canonicalize(root)?;
            let mut hash = Sha256::new();
            visit(&root, &root, Path::new(""), Some(&staging), &mut hash, &mut Vec::new())?;
            if hex::encode(hash.finalize()) != expected {
                return Err(std::io::Error::other("backend changed since deployment; deploy it again before approving"));
            }
            std::fs::rename(&staging, destination)?;
            Ok(())
        })();
        if result.is_err() { let _ = std::fs::remove_dir_all(staging); }
        result
    }).await.map_err(|e| RuntimeError::Worker(e.to_string()))?.map_err(|e| RuntimeError::Worker(e.to_string()))
}

fn visit(
    root: &Path, path: &Path, relative: &Path, destination: Option<&Path>,
    hash: &mut Sha256, parents: &mut Vec<PathBuf>,
) -> Result<(), std::io::Error> {
    let resolved = std::fs::canonicalize(path)?;
    if !resolved.starts_with(root) || parents.contains(&resolved) || parents.len() > 64 {
        return Err(std::io::Error::other(format!("backend contains an external or cyclic link: {}", relative.display())));
    }
    let meta = std::fs::metadata(&resolved)?;
    let name = relative.to_str().ok_or_else(|| std::io::Error::other("backend paths must be UTF-8"))?;
    hash.update((name.len() as u64).to_le_bytes());
    hash.update(name.as_bytes());
    if meta.is_dir() {
        hash.update(b"D");
        if let Some(dest) = destination { std::fs::create_dir_all(dest.join(relative))?; }
        parents.push(resolved.clone());
        let mut entries = std::fs::read_dir(&resolved)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            visit(root, &entry.path(), &relative.join(entry.file_name()), destination, hash, parents)?;
        }
        parents.pop();
    } else if meta.is_file() {
        let bytes = std::fs::read(&resolved)?;
        hash.update(b"F");
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(&bytes);
        if let Some(dest) = destination {
            let target = dest.join(relative);
            std::fs::write(&target, bytes)?;
            readonly(&target)?;
        }
    } else {
        return Err(std::io::Error::other(format!("backend contains a non-file artifact: {name}")));
    }
    Ok(())
}

fn readonly(path: &Path) -> Result<(), std::io::Error> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o444))
    }
    #[cfg(not(unix))]
    {
        let mut permissions = std::fs::metadata(path)?.permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(path, permissions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn failed_snapshot_leaves_no_partial_release() {
        let directory = tempfile::tempdir().unwrap();
        let backend = directory.path().join("apps/example");
        std::fs::create_dir_all(backend.join("dependency")).unwrap();
        std::fs::write(backend.join("index.js"), "import './dependency/module.js';").unwrap();
        std::fs::write(backend.join("dependency/module.js"), "export const value = 1;").unwrap();
        let digest = fingerprint(&backend).await.unwrap();
        std::fs::write(backend.join("dependency/module.js"), "export const value = 2;").unwrap();

        let result = seal(directory.path(), "example", Uuid::new_v4(), &digest).await;

        assert!(result.unwrap_err().to_string().contains("changed since deployment"));
        assert_eq!(
            std::fs::read_dir(directory.path().join("approved-backends")).unwrap().count(),
            0,
            "a rejected snapshot must not leave a partial release on disk",
        );
        assert_eq!(
            std::fs::read_to_string(backend.join("dependency/module.js")).unwrap(),
            "export const value = 2;",
            "snapshot failure must preserve the deployed backend",
        );
    }
}
