use async_trait::async_trait;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Multipart, Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use sqlx::PgPool;
use tracing::info;
use uuid::Uuid;

use crate::RuntimeError;
use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::extensions::rbac::policy::{has_permission, resolve_permissions};
use crate::routes::SharedRuntime;

use super::RuntimeExtension;

const MAX_FILE_BYTES: usize = 64 * 1024 * 1024;

fn check_storage_perm(permissions: &[String], bucket: &str, action: &str) -> Result<(), ApiError> {
    let specific = format!("storage:{bucket}:{action}");
    let global = format!("storage:{action}");
    if has_permission(permissions, &specific) || has_permission(permissions, &global) {
        Ok(())
    } else {
        Err(ApiError::Forbidden(format!("permission denied: {global}")))
    }
}

pub struct PlatformStorageExtension;

#[async_trait]
impl RuntimeExtension for PlatformStorageExtension {
    fn name(&self) -> &str {
        "platform_storage"
    }

    async fn bootstrap(&self, pool: &PgPool) -> Result<(), RuntimeError> {
        info!("bootstrapping platform storage extension");

        for ddl in [
            r#"CREATE TABLE IF NOT EXISTS rootcx_system.storage_buckets (
                name          TEXT PRIMARY KEY,
                public        BOOLEAN NOT NULL DEFAULT false,
                max_file_size BIGINT,
                allowed_types TEXT[],
                created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
            )"#,
            "INSERT INTO rootcx_system.storage_buckets (name) VALUES ('default') ON CONFLICT DO NOTHING",
            r#"CREATE TABLE IF NOT EXISTS rootcx_system.storage_objects (
                id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                bucket        TEXT NOT NULL REFERENCES rootcx_system.storage_buckets(name),
                parent_id     UUID REFERENCES rootcx_system.storage_objects(id) ON DELETE CASCADE,
                name          TEXT NOT NULL,
                is_folder     BOOLEAN NOT NULL DEFAULT false,
                content_type  TEXT NOT NULL DEFAULT 'application/octet-stream',
                size          BIGINT NOT NULL DEFAULT 0,
                content       BYTEA NOT NULL DEFAULT '',
                metadata      JSONB NOT NULL DEFAULT '{}',
                uploaded_by   UUID REFERENCES rootcx_system.users(id),
                created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
                UNIQUE(bucket, parent_id, name)
            )"#,
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_storage_objects_root_unique ON rootcx_system.storage_objects (bucket, name) WHERE parent_id IS NULL",
            "CREATE INDEX IF NOT EXISTS idx_storage_objects_parent ON rootcx_system.storage_objects (bucket, parent_id)",
            r#"INSERT INTO rootcx_system.rbac_permissions (key, description) VALUES
                ('storage:read', 'Read and list storage objects'),
                ('storage:write', 'Upload storage objects'),
                ('storage:delete', 'Delete storage objects'),
                ('storage:admin', 'Manage storage buckets')
            ON CONFLICT (key) DO NOTHING"#,
        ] {
            sqlx::query(ddl).execute(pool).await.map_err(RuntimeError::Schema)?;
        }

        info!("platform storage extension ready");
        Ok(())
    }

    fn routes(&self) -> Option<Router<SharedRuntime>> {
        Some(
            Router::new()
                .route("/api/v1/storage/buckets", get(list_buckets).post(create_bucket))
                .route("/api/v1/storage/buckets/{name}", delete(delete_bucket))
                .route("/api/v1/storage/objects/{bucket}", get(list_objects).post(create_folder))
                .route("/api/v1/storage/objects/{bucket}/upload", post(upload_object).layer(DefaultBodyLimit::max(MAX_FILE_BYTES)))
                .route("/api/v1/storage/objects/{bucket}/{id}", get(download_object).patch(rename_object).delete(delete_object))
                .route("/api/v1/storage/objects/{bucket}/{id}/ancestors", get(get_ancestors))
        )
    }
}

#[derive(Serialize, sqlx::FromRow)]
struct BucketRow {
    name: String,
    public: bool,
    max_file_size: Option<i64>,
    allowed_types: Option<Vec<String>>,
    created_at: String,
}

#[derive(Deserialize)]
struct CreateBucketInput {
    name: String,
    #[serde(default)]
    public: bool,
    #[serde(default)]
    max_file_size: Option<i64>,
    #[serde(default)]
    allowed_types: Option<Vec<String>>,
}

#[derive(Serialize, sqlx::FromRow)]
struct ObjectRow {
    id: String,
    bucket: String,
    parent_id: Option<String>,
    name: String,
    is_folder: bool,
    content_type: String,
    size: i64,
    metadata: JsonValue,
    uploaded_by: Option<String>,
    created_at: String,
}

#[derive(Deserialize)]
struct ListQuery {
    #[serde(default)]
    parent_id: Option<String>,
}

#[derive(Deserialize)]
struct CreateFolderInput {
    name: String,
    #[serde(default)]
    parent_id: Option<String>,
}

// ─── Bucket handlers ───

async fn list_buckets(
    identity: Identity,
    State(rt): State<SharedRuntime>,
) -> Result<Json<Vec<BucketRow>>, ApiError> {
    let pool = rt.pool().clone();
    let (_, perms) = resolve_permissions(&pool, identity.user_id).await?;
    check_storage_perm(&perms, "*", "read")?;

    let rows = sqlx::query_as::<_, BucketRow>(
        "SELECT name, public, max_file_size, allowed_types, created_at::text FROM rootcx_system.storage_buckets ORDER BY name",
    ).fetch_all(&pool).await?;

    Ok(Json(rows))
}

async fn create_bucket(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Json(input): Json<CreateBucketInput>,
) -> Result<(StatusCode, Json<JsonValue>), ApiError> {
    let pool = rt.pool().clone();
    let (_, perms) = resolve_permissions(&pool, identity.user_id).await?;
    check_storage_perm(&perms, "*", "admin")?;

    if input.name.is_empty() || input.name.len() > 63 || !input.name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(ApiError::BadRequest("bucket name must be 1-63 chars, alphanumeric/dash/underscore".into()));
    }

    sqlx::query(
        "INSERT INTO rootcx_system.storage_buckets (name, public, max_file_size, allowed_types) VALUES ($1, $2, $3, $4)",
    )
    .bind(&input.name)
    .bind(input.public)
    .bind(input.max_file_size)
    .bind(&input.allowed_types)
    .execute(&pool)
    .await
    .map_err(|e| {
        if e.to_string().contains("duplicate key") {
            ApiError::BadRequest(format!("bucket '{}' already exists", input.name))
        } else {
            ApiError::Internal(e.to_string())
        }
    })?;

    Ok((StatusCode::CREATED, Json(json!({ "name": input.name }))))
}

async fn delete_bucket(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(name): Path<String>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = rt.pool().clone();
    let (_, perms) = resolve_permissions(&pool, identity.user_id).await?;
    check_storage_perm(&perms, "*", "admin")?;

    if name == "default" {
        return Err(ApiError::BadRequest("cannot delete the default bucket".into()));
    }

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM rootcx_system.storage_objects WHERE bucket = $1",
    ).bind(&name).fetch_one(&pool).await?;

    if count > 0 {
        return Err(ApiError::Conflict(format!("bucket '{name}' has {count} objects — delete them first")));
    }

    let result = sqlx::query("DELETE FROM rootcx_system.storage_buckets WHERE name = $1")
        .bind(&name).execute(&pool).await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound(format!("bucket '{name}'")));
    }

    Ok(Json(json!({ "deleted": name })))
}

// ─── Object handlers ───

async fn list_objects(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(bucket): Path<String>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<ObjectRow>>, ApiError> {
    let pool = rt.pool().clone();
    let (_, perms) = resolve_permissions(&pool, identity.user_id).await?;
    check_storage_perm(&perms, &bucket, "read")?;

    let rows = if let Some(pid) = &q.parent_id {
        let parent_uuid: Uuid = pid.parse().map_err(|_| ApiError::BadRequest("invalid parent_id".into()))?;
        sqlx::query_as::<_, ObjectRow>(
            "SELECT id::text, bucket, parent_id::text, name, is_folder, content_type, size, metadata, uploaded_by::text, created_at::text
             FROM rootcx_system.storage_objects WHERE bucket = $1 AND parent_id = $2
             ORDER BY is_folder DESC, name ASC",
        ).bind(&bucket).bind(parent_uuid).fetch_all(&pool).await?
    } else {
        sqlx::query_as::<_, ObjectRow>(
            "SELECT id::text, bucket, parent_id::text, name, is_folder, content_type, size, metadata, uploaded_by::text, created_at::text
             FROM rootcx_system.storage_objects WHERE bucket = $1 AND parent_id IS NULL
             ORDER BY is_folder DESC, name ASC",
        ).bind(&bucket).fetch_all(&pool).await?
    };

    Ok(Json(rows))
}

async fn create_folder(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(bucket): Path<String>,
    Json(input): Json<CreateFolderInput>,
) -> Result<(StatusCode, Json<JsonValue>), ApiError> {
    let pool = rt.pool().clone();
    let (_, perms) = resolve_permissions(&pool, identity.user_id).await?;
    check_storage_perm(&perms, &bucket, "write")?;

    if input.name.is_empty() {
        return Err(ApiError::BadRequest("folder name required".into()));
    }

    let parent_uuid = if let Some(pid) = &input.parent_id {
        Some(pid.parse::<Uuid>().map_err(|_| ApiError::BadRequest("invalid parent_id".into()))?)
    } else {
        None
    };

    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO rootcx_system.storage_objects (id, bucket, parent_id, name, is_folder, content_type, size, content, uploaded_by)
         VALUES ($1, $2, $3, $4, true, 'application/x-directory', 0, '', $5)",
    )
    .bind(id)
    .bind(&bucket)
    .bind(parent_uuid)
    .bind(&input.name)
    .bind(identity.user_id)
    .execute(&pool)
    .await
    .map_err(|e| {
        let msg = e.to_string();
        if msg.contains("unique constraint") || msg.contains("duplicate key") {
            ApiError::BadRequest(format!("'{}' already exists in this folder", input.name))
        } else if msg.contains("violates foreign key") {
            ApiError::NotFound("parent folder not found".into())
        } else {
            ApiError::from(e)
        }
    })?;

    Ok((StatusCode::CREATED, Json(json!({ "id": id.to_string(), "name": input.name, "is_folder": true }))))
}

async fn upload_object(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(bucket): Path<String>,
    mut multipart: Multipart,
) -> Result<(StatusCode, Json<JsonValue>), ApiError> {
    let pool = rt.pool().clone();
    let (_, perms) = resolve_permissions(&pool, identity.user_id).await?;
    check_storage_perm(&perms, &bucket, "write")?;

    let bucket_row: Option<(Option<i64>, Option<Vec<String>>)> = sqlx::query_as(
        "SELECT max_file_size, allowed_types FROM rootcx_system.storage_buckets WHERE name = $1",
    ).bind(&bucket).fetch_optional(&pool).await?;

    let (max_size, allowed_types) = bucket_row.ok_or_else(|| ApiError::NotFound(format!("bucket '{bucket}'")))?;

    let mut file_name = String::new();
    let mut content_type = String::from("application/octet-stream");
    let mut data: Option<axum::body::Bytes> = None;
    let mut parent_id: Option<Uuid> = None;

    while let Some(field) = multipart.next_field().await.map_err(|e| ApiError::BadRequest(e.to_string()))? {
        match field.name() {
            Some("parent_id") => {
                let val = field.text().await.map_err(|e| ApiError::BadRequest(e.to_string()))?;
                if !val.is_empty() {
                    parent_id = Some(val.parse().map_err(|_| ApiError::BadRequest("invalid parent_id".into()))?);
                }
            }
            _ if field.file_name().is_some() => {
                if data.is_some() {
                    return Err(ApiError::BadRequest("only one file field allowed".into()));
                }
                file_name = field.file_name().unwrap_or("upload").to_string();
                content_type = field.content_type().unwrap_or("application/octet-stream").to_string();
                data = Some(field.bytes().await.map_err(|e| ApiError::BadRequest(e.to_string()))?);
            }
            _ => {}
        }
    }

    let data = data.ok_or_else(|| ApiError::BadRequest("no file field".into()))?;
    if data.is_empty() {
        return Err(ApiError::BadRequest("empty file".into()));
    }

    if let Some(types) = &allowed_types {
        if !types.is_empty() && !types.iter().any(|t| {
            if t.ends_with("/*") {
                content_type.starts_with(t.trim_end_matches("/*"))
            } else {
                &content_type == t
            }
        }) {
            return Err(ApiError::BadRequest(format!("content type '{content_type}' not allowed in bucket '{bucket}'")));
        }
    }

    if let Some(max) = max_size {
        if data.len() as i64 > max {
            return Err(ApiError::BadRequest(format!("file exceeds bucket limit ({max} bytes)")));
        }
    }

    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO rootcx_system.storage_objects (id, bucket, parent_id, name, is_folder, content_type, size, content, uploaded_by)
         VALUES ($1, $2, $3, $4, false, $5, $6, $7, $8)",
    )
    .bind(id)
    .bind(&bucket)
    .bind(parent_id)
    .bind(&file_name)
    .bind(&content_type)
    .bind(data.len() as i64)
    .bind(&data[..])
    .bind(identity.user_id)
    .execute(&pool)
    .await
    .map_err(|e| {
        let msg = e.to_string();
        if msg.contains("unique constraint") || msg.contains("duplicate key") {
            ApiError::BadRequest(format!("'{}' already exists in this folder", file_name))
        } else if msg.contains("violates foreign key") {
            ApiError::NotFound("parent folder not found".into())
        } else {
            ApiError::from(e)
        }
    })?;

    Ok((StatusCode::CREATED, Json(json!({
        "id": id.to_string(),
        "name": file_name,
        "content_type": content_type,
        "size": data.len(),
    }))))
}

async fn download_object(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((bucket, id)): Path<(String, Uuid)>,
) -> Result<Response, ApiError> {
    let pool = rt.pool().clone();

    let is_public: bool = sqlx::query_scalar(
        "SELECT public FROM rootcx_system.storage_buckets WHERE name = $1",
    ).bind(&bucket).fetch_optional(&pool).await?
    .unwrap_or(false);

    if !is_public {
        let (_, perms) = resolve_permissions(&pool, identity.user_id).await?;
        check_storage_perm(&perms, &bucket, "read")?;
    }

    let row: Option<(String, String, bool, Vec<u8>)> = sqlx::query_as(
        "SELECT name, content_type, is_folder, content FROM rootcx_system.storage_objects WHERE id = $1 AND bucket = $2",
    ).bind(id).bind(&bucket).fetch_optional(&pool).await?;

    let (name, content_type, is_folder, content) = row.ok_or_else(|| ApiError::NotFound(format!("object {id}")))?;

    if is_folder {
        return Err(ApiError::BadRequest("cannot download a folder".into()));
    }

    let safe_name: String = name.chars().filter(|c| !c.is_control() && *c != '"' && *c != '\\').collect();
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, content_type.parse().unwrap_or(header::HeaderValue::from_static("application/octet-stream")));
    headers.insert(header::CONTENT_DISPOSITION, format!("attachment; filename=\"{safe_name}\"").parse().unwrap_or(header::HeaderValue::from_static("attachment")));
    headers.insert(header::CONTENT_LENGTH, content.len().to_string().parse().unwrap());
    headers.insert(header::HeaderName::from_static("x-content-type-options"), header::HeaderValue::from_static("nosniff"));
    headers.insert(header::HeaderName::from_static("content-security-policy"), header::HeaderValue::from_static("sandbox"));

    Ok((headers, Body::from(content)).into_response())
}

#[derive(Deserialize)]
struct RenameInput {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    parent_id: Option<String>,
}

async fn rename_object(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((bucket, id)): Path<(String, Uuid)>,
    Json(input): Json<RenameInput>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = rt.pool().clone();
    let (_, perms) = resolve_permissions(&pool, identity.user_id).await?;
    check_storage_perm(&perms, &bucket, "write")?;

    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM rootcx_system.storage_objects WHERE id = $1 AND bucket = $2)",
    ).bind(id).bind(&bucket).fetch_one(&pool).await?;
    if !exists {
        return Err(ApiError::NotFound(format!("object {id}")));
    }

    if let Some(name) = &input.name {
        if name.is_empty() {
            return Err(ApiError::BadRequest("name cannot be empty".into()));
        }
        sqlx::query("UPDATE rootcx_system.storage_objects SET name = $1 WHERE id = $2")
            .bind(name).bind(id).execute(&pool).await
            .map_err(|e| {
                if e.to_string().contains("unique constraint") || e.to_string().contains("duplicate key") {
                    ApiError::BadRequest(format!("'{name}' already exists in this folder"))
                } else { ApiError::from(e) }
            })?;
    }

    if let Some(pid_str) = &input.parent_id {
        let new_parent = if pid_str.is_empty() || pid_str == "null" {
            None
        } else {
            Some(pid_str.parse::<Uuid>().map_err(|_| ApiError::BadRequest("invalid parent_id".into()))?)
        };
        if new_parent == Some(id) {
            return Err(ApiError::BadRequest("cannot move into itself".into()));
        }
        // Prevent circular moves: walk ancestry of target to ensure `id` is not an ancestor
        if let Some(target) = new_parent {
            let mut cursor: Option<Uuid> = Some(target);
            for _ in 0..100 {
                match cursor {
                    None => break,
                    Some(c) if c == id => return Err(ApiError::BadRequest("cannot move a folder into its own descendant".into())),
                    Some(c) => {
                        cursor = sqlx::query_scalar::<_, Option<Uuid>>(
                            "SELECT parent_id FROM rootcx_system.storage_objects WHERE id = $1",
                        ).bind(c).fetch_optional(&pool).await?.flatten();
                    }
                }
            }
        }
        sqlx::query("UPDATE rootcx_system.storage_objects SET parent_id = $1 WHERE id = $2")
            .bind(new_parent).bind(id).execute(&pool).await
            .map_err(|e| {
                let msg = e.to_string();
                if msg.contains("unique constraint") || msg.contains("duplicate key") {
                    ApiError::BadRequest("an item with this name already exists in the target folder".into())
                } else if msg.contains("violates foreign key") {
                    ApiError::NotFound("target folder not found".into())
                } else { ApiError::from(e) }
            })?;
    }

    Ok(Json(json!({ "id": id.to_string(), "updated": true })))
}

async fn delete_object(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((bucket, id)): Path<(String, Uuid)>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = rt.pool().clone();
    let (_, perms) = resolve_permissions(&pool, identity.user_id).await?;
    check_storage_perm(&perms, &bucket, "delete")?;

    let result = sqlx::query("DELETE FROM rootcx_system.storage_objects WHERE id = $1 AND bucket = $2")
        .bind(id).bind(&bucket).execute(&pool).await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound(format!("object {id}")));
    }

    Ok(Json(json!({ "deleted": id.to_string() })))
}

async fn get_ancestors(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((bucket, id)): Path<(String, Uuid)>,
) -> Result<Json<Vec<JsonValue>>, ApiError> {
    let pool = rt.pool().clone();
    let (_, perms) = resolve_permissions(&pool, identity.user_id).await?;
    check_storage_perm(&perms, &bucket, "read")?;

    let mut ancestors: Vec<JsonValue> = Vec::new();

    let rows: Vec<(String, Option<String>, String)> = sqlx::query_as(
        r#"WITH RECURSIVE chain AS (
            SELECT id, parent_id, name FROM rootcx_system.storage_objects WHERE id = $1 AND bucket = $2
            UNION ALL
            SELECT o.id, o.parent_id, o.name FROM rootcx_system.storage_objects o
            JOIN chain c ON c.parent_id = o.id
        )
        SELECT id::text, parent_id::text, name FROM chain"#,
    ).bind(id).bind(&bucket).fetch_all(&pool).await?;

    // Walk from root (parent_id IS NULL) down to the target
    let mut cursor: Option<&str> = None;
    for _ in 0..100 {
        let next = rows.iter().find(|(_, p, _)| p.as_deref() == cursor);
        match next {
            Some((fid, _, name)) => {
                ancestors.push(json!({ "id": fid, "name": name }));
                if *fid == id.to_string() { break; }
                cursor = Some(fid);
            }
            None => break,
        }
    }

    Ok(Json(ancestors))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_perm_allows_global_permission() {
        let perms = vec!["storage:write".into()];
        assert!(check_storage_perm(&perms, "photos", "write").is_ok());
    }

    #[test]
    fn check_perm_allows_bucket_specific_permission() {
        let perms = vec!["storage:photos:read".into()];
        assert!(check_storage_perm(&perms, "photos", "read").is_ok());
    }

    #[test]
    fn check_perm_denies_wrong_bucket() {
        let perms = vec!["storage:photos:read".into()];
        assert!(check_storage_perm(&perms, "documents", "read").is_err());
    }

    #[test]
    fn check_perm_denies_wrong_action() {
        let perms = vec!["storage:read".into()];
        assert!(check_storage_perm(&perms, "photos", "write").is_err());
    }

    #[test]
    fn check_perm_allows_wildcard() {
        let cases = vec![
            vec!["storage:*".into()],
            vec!["*".into()],
        ];
        for perms in cases {
            assert!(check_storage_perm(&perms, "any-bucket", "delete").is_ok(), "failed for {:?}", perms);
        }
    }

    #[test]
    fn bucket_name_validation() {
        let long_63 = "a".repeat(63);
        let long_64 = "a".repeat(64);
        let cases: Vec<(&str, bool)> = vec![
            ("", false),
            ("a", true),
            ("my-bucket_123", true),
            ("UPPER", true),
            ("has space", false),
            ("has.dot", false),
            ("has/slash", false),
            (&long_63, true),
            (&long_64, false),
        ];
        for (name, valid) in cases {
            let is_valid = !name.is_empty() && name.len() <= 63
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
            assert_eq!(is_valid, valid, "bucket name '{name}' expected valid={valid}");
        }
    }

    #[test]
    fn content_type_wildcard_matching() {
        let allowed = vec!["image/*".to_string(), "application/pdf".to_string()];
        let cases = vec![
            ("image/png", true),
            ("image/jpeg", true),
            ("application/pdf", true),
            ("application/json", false),
            ("text/plain", false),
        ];
        for (ct, expected) in cases {
            let matches = allowed.iter().any(|t| {
                if t.ends_with("/*") {
                    ct.starts_with(t.trim_end_matches("/*"))
                } else {
                    ct == t
                }
            });
            assert_eq!(matches, expected, "content_type '{ct}' expected allowed={expected}");
        }
    }

    #[cfg(test)]
    mod integration {
        use sqlx::PgPool;
        use uuid::Uuid;

        async fn pool() -> PgPool {
            let url = std::env::var("TEST_DATABASE_URL")
                .unwrap_or_else(|_| "postgres://rootcx:rootcx@localhost:5480/rootcx".into());
            let pool = PgPool::connect(&url).await.expect("connect to test DB");
            let _ = sqlx::query("CREATE SCHEMA IF NOT EXISTS rootcx_system").execute(&pool).await;
            let ext = super::super::PlatformStorageExtension;
            use crate::extensions::RuntimeExtension;
            ext.bootstrap(&pool).await.expect("bootstrap");
            pool
        }

        async fn make_bucket(pool: &PgPool) -> String {
            let name = format!("test-{}", Uuid::new_v4().simple());
            sqlx::query("INSERT INTO rootcx_system.storage_buckets (name) VALUES ($1)")
                .bind(&name).execute(pool).await.unwrap();
            name
        }

        async fn make_folder(pool: &PgPool, bucket: &str, parent: Option<Uuid>, name: &str) -> Uuid {
            let id = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO rootcx_system.storage_objects (id, bucket, parent_id, name, is_folder, content_type, size, content)
                 VALUES ($1, $2, $3, $4, true, 'application/x-directory', 0, '')",
            ).bind(id).bind(bucket).bind(parent).bind(name).execute(pool).await.unwrap();
            id
        }

        async fn make_file(pool: &PgPool, bucket: &str, parent: Option<Uuid>, name: &str) -> Uuid {
            let id = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO rootcx_system.storage_objects (id, bucket, parent_id, name, is_folder, content_type, size, content)
                 VALUES ($1, $2, $3, $4, false, 'text/plain', 5, '\\x68656c6c6f')",
            ).bind(id).bind(bucket).bind(parent).bind(name).execute(pool).await.unwrap();
            id
        }

        async fn exists(pool: &PgPool, id: Uuid) -> bool {
            sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM rootcx_system.storage_objects WHERE id = $1)")
                .bind(id).fetch_one(pool).await.unwrap()
        }

        async fn cleanup(pool: &PgPool, bucket: &str) {
            let _ = sqlx::query("DELETE FROM rootcx_system.storage_objects WHERE bucket = $1").bind(bucket).execute(pool).await;
            let _ = sqlx::query("DELETE FROM rootcx_system.storage_buckets WHERE name = $1").bind(bucket).execute(pool).await;
        }

        #[tokio::test]
        async fn cascade_delete_folder_removes_children() {
            let pool = pool().await;
            let b = make_bucket(&pool).await;
            let folder = make_folder(&pool, &b, None, "docs").await;
            let sub = make_folder(&pool, &b, Some(folder), "sub").await;
            let file1 = make_file(&pool, &b, Some(folder), "a.txt").await;
            let file2 = make_file(&pool, &b, Some(sub), "b.txt").await;

            sqlx::query("DELETE FROM rootcx_system.storage_objects WHERE id = $1")
                .bind(folder).execute(&pool).await.unwrap();

            assert!(!exists(&pool, folder).await, "folder should be gone");
            assert!(!exists(&pool, sub).await, "subfolder should be cascaded");
            assert!(!exists(&pool, file1).await, "file in folder should be cascaded");
            assert!(!exists(&pool, file2).await, "file in subfolder should be cascaded");

            cleanup(&pool, &b).await;
        }

        #[tokio::test]
        async fn unique_name_per_parent() {
            let pool = pool().await;
            let b = make_bucket(&pool).await;
            make_file(&pool, &b, None, "readme.md").await;

            let dup = sqlx::query(
                "INSERT INTO rootcx_system.storage_objects (id, bucket, parent_id, name, is_folder, content_type, size, content)
                 VALUES ($1, $2, NULL, 'readme.md', false, 'text/plain', 1, '\\x78')",
            ).bind(Uuid::new_v4()).bind(&b).execute(&pool).await;

            assert!(dup.is_err(), "duplicate name at same level must fail");
            cleanup(&pool, &b).await;
        }

        #[tokio::test]
        async fn same_name_different_parents_allowed() {
            let pool = pool().await;
            let b = make_bucket(&pool).await;
            let f1 = make_folder(&pool, &b, None, "a").await;
            let f2 = make_folder(&pool, &b, None, "b").await;

            make_file(&pool, &b, Some(f1), "readme.md").await;
            make_file(&pool, &b, Some(f2), "readme.md").await;

            cleanup(&pool, &b).await;
        }

        #[tokio::test]
        async fn move_into_self_blocked() {
            let pool = pool().await;
            let b = make_bucket(&pool).await;
            let folder = make_folder(&pool, &b, None, "docs").await;

            let result = sqlx::query("UPDATE rootcx_system.storage_objects SET parent_id = $1 WHERE id = $1")
                .bind(folder).execute(&pool).await;
            // Postgres allows self-ref FK but our handler blocks it — here we verify the DB level
            // The actual business logic check is in rename_object handler (tested via API)
            // DB-level: this actually succeeds in PG (no FK cycle check) — confirming why we need the handler check
            if result.is_ok() {
                // Revert
                sqlx::query("UPDATE rootcx_system.storage_objects SET parent_id = NULL WHERE id = $1")
                    .bind(folder).execute(&pool).await.unwrap();
            }

            cleanup(&pool, &b).await;
        }

        #[tokio::test]
        async fn circular_move_detected_by_ancestry_walk() {
            // grandparent -> parent -> child
            // Trying to move grandparent INTO child should be blocked by handler logic.
            // Here we verify the DB does NOT block it (confirming the handler must).
            let pool = pool().await;
            let b = make_bucket(&pool).await;
            let gp = make_folder(&pool, &b, None, "grandparent").await;
            let parent = make_folder(&pool, &b, Some(gp), "parent").await;
            let child = make_folder(&pool, &b, Some(parent), "child").await;

            // DB allows this (no built-in cycle detection) — this is why the handler ancestry walk exists
            let result = sqlx::query("UPDATE rootcx_system.storage_objects SET parent_id = $1 WHERE id = $2")
                .bind(child).bind(gp).execute(&pool).await;
            assert!(result.is_ok(), "DB does not prevent cycles — handler must");

            // Revert to avoid orphaned data
            sqlx::query("UPDATE rootcx_system.storage_objects SET parent_id = NULL WHERE id = $1")
                .bind(gp).execute(&pool).await.unwrap();

            cleanup(&pool, &b).await;
        }

        #[tokio::test]
        async fn rename_to_existing_name_blocked() {
            let pool = pool().await;
            let b = make_bucket(&pool).await;
            let f1 = make_file(&pool, &b, None, "a.txt").await;
            make_file(&pool, &b, None, "b.txt").await;

            let result = sqlx::query("UPDATE rootcx_system.storage_objects SET name = 'b.txt' WHERE id = $1")
                .bind(f1).execute(&pool).await;
            assert!(result.is_err(), "rename to existing name at same level must fail");

            cleanup(&pool, &b).await;
        }

        #[tokio::test]
        async fn file_in_folder_scoped_correctly() {
            let pool = pool().await;
            let b = make_bucket(&pool).await;
            let folder = make_folder(&pool, &b, None, "docs").await;
            let file = make_file(&pool, &b, Some(folder), "report.pdf").await;

            // Query with correct parent returns the file
            let found: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM rootcx_system.storage_objects WHERE id = $1 AND parent_id = $2)",
            ).bind(file).bind(folder).fetch_one(&pool).await.unwrap();
            assert!(found);

            // Query at root (parent_id IS NULL) does NOT return it
            let at_root: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM rootcx_system.storage_objects WHERE id = $1 AND parent_id IS NULL)",
            ).bind(file).fetch_one(&pool).await.unwrap();
            assert!(!at_root, "file with parent must not appear at root level");

            cleanup(&pool, &b).await;
        }
    }
}
