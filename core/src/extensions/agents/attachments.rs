use crate::extensions::storage::backend::{PostgresBackend, StorageBackend};
use crate::{api_error::ApiError, ipc::FileAttachment, routes::SharedRuntime};
use axum::body::Bytes;
use uuid::Uuid;

pub(crate) const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;
pub(crate) const MAX_ATTACHMENTS: usize = 10;

pub(crate) fn validate_count(count: usize) -> Result<(), ApiError> {
    if count > MAX_ATTACHMENTS {
        return Err(ApiError::BadRequest("too many agent attachments".into()));
    }
    Ok(())
}

pub(crate) fn validate(bytes: &[u8], content_type: &str) -> Result<(), ApiError> {
    let limit = if content_type.starts_with("image/") {
        MAX_IMAGE_BYTES
    } else {
        crate::extensions::storage::max_file_bytes()
    };
    if bytes.is_empty() || bytes.len() > limit {
        return Err(ApiError::BadRequest("invalid attachment size".into()));
    }
    let valid = match content_type {
        "image/png" => bytes.starts_with(&[137, 80, 78, 71, 13, 10, 26, 10]),
        "image/jpeg" => bytes.starts_with(&[255, 216, 255]),
        mime if mime.starts_with("image/") => false,
        _ => true,
    };
    if !valid {
        return Err(ApiError::BadRequest(
            "invalid agent image: expected JPEG or PNG".into(),
        ));
    }
    Ok(())
}

pub(crate) async fn read_download(
    mut response: reqwest::Response,
    content_type: &str,
) -> Option<Bytes> {
    if !response.status().is_success() {
        return None;
    }
    let limit = if content_type.starts_with("image/") {
        MAX_IMAGE_BYTES
    } else {
        crate::extensions::storage::max_file_bytes()
    };
    if response
        .content_length()
        .is_some_and(|size| size > limit as u64)
    {
        return None;
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if chunk.len() > limit.saturating_sub(bytes.len()) {
            return None;
        }
        bytes.extend_from_slice(&chunk);
    }
    validate(&bytes, content_type).ok()?;
    Some(Bytes::from(bytes))
}

fn download(
    rt: &SharedRuntime,
    app_id: &str,
    file_id: Uuid,
    name: String,
    content_type: String,
) -> FileAttachment {
    let nonce = rt
        .upload_nonces()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .create_download(file_id, app_id);
    FileAttachment {
        name,
        content_type,
        url: crate::extensions::storage::download_url(rt.runtime_url(), &nonce),
    }
}

pub(crate) async fn stored(
    rt: &SharedRuntime,
    app_id: &str,
    human: Uuid,
    ids: &[String],
) -> Result<Option<Vec<FileAttachment>>, ApiError> {
    validate_count(ids.len())?;
    if ids.is_empty() {
        return Ok(None);
    }
    crate::governance::authority::require_perm(
        rt.pool(),
        human,
        &format!("app:{app_id}:storage.read"),
    )
    .await?;
    let mut files = Vec::with_capacity(ids.len());
    for raw_id in ids {
        let id = raw_id
            .parse::<Uuid>()
            .map_err(|_| ApiError::BadRequest("invalid attachment file ID".into()))?;
        let (name, content_type, size): (String, String, i64) = sqlx::query_as(
            "SELECT name, content_type, size FROM rootcx_system.files WHERE id = $1 AND app_id = $2",
        ).bind(id).bind(app_id).fetch_optional(rt.pool()).await?
            .ok_or_else(|| ApiError::NotFound(format!("file {id}")))?;
        if content_type.starts_with("image/") {
            if size <= 0 || size > MAX_IMAGE_BYTES as i64 {
                return Err(ApiError::BadRequest("invalid attachment size".into()));
            }
            let object = PostgresBackend.get(rt.pool(), id, app_id).await?;
            validate(&object.content, &content_type)?;
        }
        files.push((id, name, content_type));
    }
    Ok(Some(
        files
            .into_iter()
            .map(|(id, name, mime)| download(rt, app_id, id, name, mime))
            .collect(),
    ))
}

pub(crate) async fn received(
    rt: &SharedRuntime,
    app_id: &str,
    human: Uuid,
    files: Vec<(Bytes, String, String)>,
) -> Result<Option<Vec<FileAttachment>>, ApiError> {
    validate_count(files.len())?;
    if files.is_empty() {
        return Ok(None);
    }
    // Validate the entire delivery before storing or issuing capabilities.
    for (bytes, mime, _) in &files {
        validate(bytes, mime)?;
    }
    let mut stored = Vec::with_capacity(files.len());
    for (bytes, mime, name) in files {
        let id = Uuid::new_v4();
        PostgresBackend
            .put(rt.pool(), id, app_id, &name, &mime, &bytes, Some(human))
            .await?;
        stored.push((id, name, mime));
    }
    Ok(Some(
        stored
            .into_iter()
            .map(|(id, name, mime)| download(rt, app_id, id, name, mime))
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn agent_images_reject_mismatched_unbounded_and_unsupported_content() {
        let png = [137, 80, 78, 71, 13, 10, 26, 10];
        for (bytes, mime) in [
            (vec![], "image/png"),
            (png.to_vec(), "image/jpeg"),
            (b"<svg/>".to_vec(), "image/svg+xml"),
            (vec![0; MAX_IMAGE_BYTES + 1], "image/png"),
        ] {
            assert!(
                validate(&bytes, mime).is_err(),
                "{mime}, {} bytes",
                bytes.len()
            );
        }
        assert!(validate(&png, "image/png").is_ok());
        assert!(validate(b"%PDF-1.4", "application/pdf").is_ok());
        assert!(validate_count(MAX_ATTACHMENTS).is_ok());
        assert!(validate_count(MAX_ATTACHMENTS + 1).is_err());
    }
}
