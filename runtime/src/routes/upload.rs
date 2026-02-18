use axum::extract::{Multipart, Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde_json::{json, Value as JsonValue};
use tokio::io::AsyncWriteExt;

use crate::api_error::ApiError;
use super::SharedRuntime;

const ALLOWED_EXTENSIONS: &[&str] = &[
    "csv", "json", "xml", "txt", "pdf", "png", "jpg", "jpeg", "gif", "webp",
    "svg", "xlsx", "xls", "doc", "docx", "zip", "gz", "parquet",
];

pub async fn upload_file(State(rt): State<SharedRuntime>, Path(app_id): Path<String>, mut multipart: Multipart) -> Result<(StatusCode, Json<JsonValue>), ApiError> {
    let data_dir = rt.lock().await.data_dir().to_path_buf();
    let uploads_dir = data_dir.join("uploads").join(&app_id);
    tokio::fs::create_dir_all(&uploads_dir).await.map_err(|e| ApiError::Internal(e.to_string()))?;

    let mut field = multipart.next_field().await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?
        .ok_or_else(|| ApiError::BadRequest("no file field".into()))?;

    let name = field.file_name().unwrap_or("upload").to_string();
    let ext = std::path::Path::new(&name)
        .extension().and_then(|e| e.to_str()).unwrap_or("bin");

    if !ALLOWED_EXTENSIONS.contains(&ext.to_lowercase().as_str()) {
        return Err(ApiError::BadRequest(format!("file extension '.{ext}' not allowed")));
    }

    let file_id = uuid::Uuid::new_v4();
    let dest = uploads_dir.join(format!("{file_id}.{ext}"));

    // Stream to disk to avoid buffering full payload in memory
    let mut file = tokio::fs::File::create(&dest).await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let mut size: usize = 0;
    while let Some(chunk) = field.chunk().await.map_err(|e| ApiError::BadRequest(e.to_string()))? {
        size += chunk.len();
        file.write_all(&chunk).await.map_err(|e| ApiError::Internal(e.to_string()))?;
    }
    file.flush().await.map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok((StatusCode::CREATED, Json(json!({
        "file_id": file_id.to_string(),
        "original_name": name,
        "size": size,
    }))))
}
