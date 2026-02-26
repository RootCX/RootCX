use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use uuid::Uuid;

use super::policy::{PolicyCache, require_admin, resolve_user_roles};
use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::routes::{self, SharedRuntime};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RoleResponse {
    name: String,
    description: Option<String>,
    inherits: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AssignmentResponse {
    user_id: Uuid,
    role: String,
    assigned_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RoleAssignment {
    user_id: Uuid,
    role: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EffectivePermissions {
    roles: Vec<String>,
    permissions: HashMap<String, EntityPermission>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EntityPermission {
    actions: Vec<String>,
    ownership: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PolicyResponse {
    role: String,
    entity: String,
    actions: Vec<String>,
    ownership: bool,
}

pub(crate) async fn list_policies(
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
) -> Result<Json<Vec<PolicyResponse>>, ApiError> {
    let pool = routes::pool(&rt).await?;
    let rows: Vec<(String, String, Vec<String>, bool)> = sqlx::query_as(
        "SELECT role, entity, actions, ownership FROM rootcx_system.rbac_policies WHERE app_id = $1 ORDER BY role, entity",
    )
    .bind(&app_id)
    .fetch_all(&pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|(role, entity, actions, ownership)| PolicyResponse { role, entity, actions, ownership })
            .collect(),
    ))
}

pub(crate) async fn list_roles(
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
) -> Result<Json<Vec<RoleResponse>>, ApiError> {
    let pool = routes::pool(&rt).await?;
    let rows: Vec<(String, Option<String>, Vec<String>)> = sqlx::query_as(
        "SELECT name, description, inherits FROM rootcx_system.rbac_roles WHERE app_id = $1 ORDER BY name",
    )
    .bind(&app_id)
    .fetch_all(&pool)
    .await?;
    Ok(Json(
        rows.into_iter().map(|(name, description, inherits)| RoleResponse { name, description, inherits }).collect(),
    ))
}

pub(crate) async fn list_assignments(
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
    identity: Identity,
    axum::Extension(cache): axum::Extension<Arc<PolicyCache>>,
) -> Result<Json<Vec<AssignmentResponse>>, ApiError> {
    let pool = routes::pool(&rt).await?;
    require_admin(&pool, &cache, &app_id, identity.user_id).await?;
    let rows: Vec<(Uuid, String, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT user_id, role, assigned_at FROM rootcx_system.rbac_assignments WHERE app_id = $1 ORDER BY assigned_at DESC",
    ).bind(&app_id).fetch_all(&pool).await?;
    Ok(Json(
        rows.into_iter()
            .map(|(user_id, role, assigned_at)| AssignmentResponse { user_id, role, assigned_at })
            .collect(),
    ))
}

pub(crate) async fn assign_role(
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
    identity: Identity,
    axum::Extension(cache): axum::Extension<Arc<PolicyCache>>,
    Json(body): Json<RoleAssignment>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt).await?;
    require_admin(&pool, &cache, &app_id, identity.user_id).await?;

    let exists: Option<(String,)> =
        sqlx::query_as("SELECT name FROM rootcx_system.rbac_roles WHERE app_id = $1 AND name = $2")
            .bind(&app_id)
            .bind(&body.role)
            .fetch_optional(&pool)
            .await?;
    if exists.is_none() {
        return Err(ApiError::BadRequest(format!("role '{}' not defined for app '{app_id}'", body.role)));
    }

    sqlx::query(
        "INSERT INTO rootcx_system.rbac_assignments (user_id, app_id, role) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
    )
    .bind(body.user_id)
    .bind(&app_id)
    .bind(&body.role)
    .execute(&pool)
    .await?;

    Ok(Json(json!({ "message": format!("role '{}' assigned", body.role) })))
}

pub(crate) async fn revoke_role(
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
    identity: Identity,
    axum::Extension(cache): axum::Extension<Arc<PolicyCache>>,
    Json(body): Json<RoleAssignment>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt).await?;
    require_admin(&pool, &cache, &app_id, identity.user_id).await?;

    let r = sqlx::query(
        "DELETE FROM rootcx_system.rbac_assignments \
         WHERE user_id = $1 AND app_id = $2 AND role = $3 \
         AND (role != 'admin' OR \
              (SELECT COUNT(*) FROM rootcx_system.rbac_assignments WHERE app_id = $2 AND role = 'admin') > 1)",
    )
    .bind(body.user_id)
    .bind(&app_id)
    .bind(&body.role)
    .execute(&pool)
    .await?;

    if r.rows_affected() == 0 {
        let is_last_admin = body.role == "admin"
            && sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM rootcx_system.rbac_assignments WHERE app_id = $1 AND role = 'admin'")
                .bind(&app_id).fetch_one(&pool).await? <= 1;
        return Err(if is_last_admin {
            ApiError::BadRequest("cannot revoke the last admin of an app".into())
        } else {
            ApiError::NotFound(format!("assignment not found for role '{}'", body.role))
        });
    }
    Ok(Json(json!({ "message": format!("role '{}' revoked", body.role) })))
}

pub(crate) async fn my_permissions(
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
    identity: Identity,
    axum::Extension(cache): axum::Extension<Arc<PolicyCache>>,
) -> Result<Json<EffectivePermissions>, ApiError> {
    let pool = routes::pool(&rt).await?;
    compute_permissions(&pool, &cache, &app_id, identity.user_id).await
}

pub(crate) async fn user_permissions(
    State(rt): State<SharedRuntime>,
    Path((app_id, target_user_id)): Path<(String, Uuid)>,
    identity: Identity,
    axum::Extension(cache): axum::Extension<Arc<PolicyCache>>,
) -> Result<Json<EffectivePermissions>, ApiError> {
    let pool = routes::pool(&rt).await?;
    if identity.user_id != target_user_id {
        require_admin(&pool, &cache, &app_id, identity.user_id).await?;
    }
    compute_permissions(&pool, &cache, &app_id, target_user_id).await
}

async fn compute_permissions(
    pool: &sqlx::PgPool,
    cache: &Arc<PolicyCache>,
    app_id: &str,
    user_id: Uuid,
) -> Result<Json<EffectivePermissions>, ApiError> {
    let Some(cached) = cache.get_or_fetch(pool, app_id).await? else {
        return Ok(Json(EffectivePermissions { roles: vec![], permissions: HashMap::new() }));
    };

    let expanded = resolve_user_roles(pool, &cached, user_id, app_id).await?;

    let mut perms: HashMap<String, EntityPermission> = HashMap::new();
    for p in &cached.policies {
        if !expanded.contains(&p.role) {
            continue;
        }
        let entry =
            perms.entry(p.entity.clone()).or_insert_with(|| EntityPermission { actions: Vec::new(), ownership: true });
        for action in &p.actions {
            if !entry.actions.contains(action) {
                entry.actions.push(action.clone());
            }
        }
        // Matches evaluate() semantics: any unrestricted grant lifts the ownership constraint.
        entry.ownership &= p.ownership;
    }

    Ok(Json(EffectivePermissions { roles: expanded.into_iter().collect(), permissions: perms }))
}
