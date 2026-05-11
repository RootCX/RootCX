//! Owner-side endpoints for public shares.
//!
//! All routes require a normal JWT (`Identity` extractor) and the
//! `app:{app_id}:public.share` permission. App developers add this permission
//! to their manifest's `permissions` block, then assign a role that holds it
//! to whichever users should be able to publish content.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::extensions::rbac::policy::{has_permission, resolve_permissions};
use crate::routes::{SharedRuntime, pool};

use super::tokens;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateShareRequest {
    #[serde(default)]
    pub context: JsonValue,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateShareResponse {
    pub id: Uuid,
    pub url: String,
    /// Raw token. Only returned at creation time — never stored or re-served.
    pub token: String,
    pub token_prefix: String,
    pub context: JsonValue,
    pub created_at: DateTime<Utc>,
    pub revoked: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareListing {
    pub id: Uuid,
    pub app_id: String,
    pub context: JsonValue,
    pub token_prefix: String,
    pub created_at: DateTime<Utc>,
    pub last_accessed_at: Option<DateTime<Utc>>,
    pub access_count: i64,
}

/// Check that the calling user holds `app:{app_id}:public.share`.
async fn require_share_perm(pool: &PgPool, identity: &Identity, app_id: &str) -> Result<(), ApiError> {
    let (_, perms) = resolve_permissions(pool, identity.user_id).await?;
    let key = format!("app:{app_id}:public.share");
    if has_permission(&perms, &key) {
        Ok(())
    } else {
        Err(ApiError::Forbidden(format!("permission denied: {key}")))
    }
}

/// Build the public share URL from the runtime's base URL and the raw token.
fn build_share_url(runtime_url: &str, token: &str) -> String {
    // The runtime serves the share landing page on the app side (per-app
    // frontend route `/share/:token`). The runtime URL is whatever the
    // caller will hit, so we just append the path. The app frontend handles
    // resolution from there.
    let trimmed = runtime_url.trim_end_matches('/');
    format!("{trimmed}/share/{token}")
}

/// POST /api/v1/apps/{app_id}/public-shares
///
/// Body: `{ context }` — opaque to the core, the app puts whatever scope keys
/// it needs (e.g. `{ "board_id": "..." }`).
///
/// Returns the freshly minted share with the raw token. The token is shown
/// **once**; subsequent GETs only expose the prefix.
///
/// Idempotent on `(app_id, created_by, context)`: if an active share already
/// exists for the calling user with the same context, returns it (with the
/// raw token re-generated only on first creation — we cannot recover an
/// existing token because we only store its hash).
pub async fn create_share(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
    Json(body): Json<CreateShareRequest>,
) -> Result<(StatusCode, Json<CreateShareResponse>), ApiError> {
    let pool = pool(&rt);
    require_share_perm(&pool, &identity, &app_id).await?;

    // Idempotence: do we already have an active share for this triple?
    // We can't return the raw token (we don't have it) — but we can tell the
    // caller "already shared" so the UI can offer to revoke first. To keep
    // things simple, we treat the response shape as identical but with
    // `token` empty when we hit this branch. Most front-ends will store the
    // token at the first creation; if they lost it, they should revoke and
    // recreate.
    let existing: Option<(Uuid, String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT id, token_prefix, created_at \
           FROM rootcx_system.public_shares \
          WHERE app_id = $1 AND created_by = $2 \
            AND md5(context::text) = md5($3::jsonb::text) \
            AND revoked_at IS NULL",
    )
    .bind(&app_id)
    .bind(identity.user_id)
    .bind(&body.context)
    .fetch_optional(&pool)
    .await?;

    if let Some((id, token_prefix, created_at)) = existing {
        return Ok((
            StatusCode::OK,
            Json(CreateShareResponse {
                id,
                url: build_share_url(rt.runtime_url(), ""),
                token: String::new(),
                token_prefix,
                context: body.context,
                created_at,
                revoked: false,
            }),
        ));
    }

    let raw = tokens::generate();
    let hash = tokens::hash(&raw);
    let prefix = tokens::prefix(&raw);

    let row: (Uuid, DateTime<Utc>) = sqlx::query_as(
        "INSERT INTO rootcx_system.public_shares \
           (app_id, token_hash, token_prefix, context, created_by) \
         VALUES ($1, $2, $3, $4, $5) \
         RETURNING id, created_at",
    )
    .bind(&app_id)
    .bind(&hash[..])
    .bind(&prefix)
    .bind(&body.context)
    .bind(identity.user_id)
    .fetch_one(&pool)
    .await?;

    let (id, created_at) = row;
    let url = build_share_url(rt.runtime_url(), &raw);

    Ok((
        StatusCode::CREATED,
        Json(CreateShareResponse {
            id,
            url,
            token: raw,
            token_prefix: prefix,
            context: body.context,
            created_at,
            revoked: false,
        }),
    ))
}

/// GET /api/v1/apps/{app_id}/public-shares
///
/// Lists the caller's active shares for the given app. Token values are
/// never returned — only the 8-char prefix for UI display.
pub async fn list_shares(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
) -> Result<Json<Vec<ShareListing>>, ApiError> {
    let pool = pool(&rt);
    require_share_perm(&pool, &identity, &app_id).await?;

    let rows: Vec<(Uuid, String, JsonValue, String, DateTime<Utc>, Option<DateTime<Utc>>, i64)> =
        sqlx::query_as(
            "SELECT id, app_id, context, token_prefix, created_at, last_accessed_at, access_count \
               FROM rootcx_system.public_shares \
              WHERE app_id = $1 AND created_by = $2 AND revoked_at IS NULL \
              ORDER BY created_at DESC",
        )
        .bind(&app_id)
        .bind(identity.user_id)
        .fetch_all(&pool)
        .await?;

    Ok(Json(
        rows.into_iter()
            .map(|(id, app_id, context, token_prefix, created_at, last_accessed_at, access_count)| {
                ShareListing { id, app_id, context, token_prefix, created_at, last_accessed_at, access_count }
            })
            .collect(),
    ))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareInfo {
    pub app_id: String,
    pub context: JsonValue,
}

/// GET /api/v1/public/share/info
///
/// Bearer = share token (NOT a JWT). Anonymous endpoint — anyone with the
/// token can read its `app_id` and `context`. No JWT, no RBAC. The whole
/// point of this endpoint is to let a `/share/:token` frontend look up which
/// app and which record it should render without baking the id into the URL.
///
/// Implementation note: we use `CallerAuth` to leverage the existing
/// resolve-token path. Non-share callers (JWT users, anonymous, malformed
/// tokens) are all rejected uniformly to avoid leaking which case occurred.
pub async fn share_info(
    auth: crate::extensions::sharing::guard::CallerAuth,
) -> Result<Json<ShareInfo>, ApiError> {
    use crate::extensions::sharing::guard::CallerAuth;
    match auth {
        CallerAuth::ShareToken(share) => Ok(Json(ShareInfo {
            app_id: share.app_id,
            context: share.context,
        })),
        _ => Err(ApiError::Unauthorized("share token required".into())),
    }
}

/// DELETE /api/v1/apps/{app_id}/public-shares/{share_id}
///
/// Revokes the share. Filtered by `created_by` — non-creators get 404 (we
/// don't leak whether the share id exists).
pub async fn revoke_share(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, share_id)): Path<(String, Uuid)>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = pool(&rt);
    require_share_perm(&pool, &identity, &app_id).await?;

    let result = sqlx::query(
        "UPDATE rootcx_system.public_shares \
            SET revoked_at = now() \
          WHERE id = $1 AND app_id = $2 AND created_by = $3 AND revoked_at IS NULL",
    )
    .bind(share_id)
    .bind(&app_id)
    .bind(identity.user_id)
    .execute(&pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound(format!("share '{share_id}' not found")));
    }

    Ok(Json(json!({ "message": format!("share '{share_id}' revoked") })))
}
