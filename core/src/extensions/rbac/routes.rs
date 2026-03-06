use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, State};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use uuid::Uuid;

use super::policy::{detect_cycle, require_admin, resolve_permissions};
use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::routes::{self, SharedRuntime};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RoleResponse {
    name: String,
    description: Option<String>,
    inherits: Vec<String>,
    permissions: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AssignmentResponse {
    user_id: Uuid,
    role: String,
    assigned_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EffectivePermissions {
    roles: Vec<String>,
    permissions: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PermissionDeclarationResponse {
    key: String,
    description: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RoleAssignment {
    user_id: Uuid,
    role: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateRoleRequest {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    inherits: Vec<String>,
    #[serde(default)]
    permissions: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateRoleRequest {
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    inherits: Option<Vec<String>>,
    #[serde(default)]
    permissions: Option<Vec<String>>,
}

fn validate_hierarchy(pool_roles: Vec<(String, Vec<String>)>, name: &str, inherits: &[String]) -> Result<(), ApiError> {
    let mut role_map: HashMap<String, Vec<String>> = pool_roles.into_iter().collect();
    role_map.insert(name.to_string(), inherits.to_vec());
    if let Some(cycle) = detect_cycle(&role_map) {
        return Err(ApiError::BadRequest(format!("role hierarchy cycle involving '{cycle}'")));
    }
    Ok(())
}

pub(crate) async fn list_roles(
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
) -> Result<Json<Vec<RoleResponse>>, ApiError> {
    let pool = routes::pool(&rt).await?;
    let rows: Vec<(String, Option<String>, Vec<String>, Vec<String>)> = sqlx::query_as(
        "SELECT name, description, inherits, permissions FROM rootcx_system.rbac_roles WHERE app_id = $1 ORDER BY name",
    ).bind(&app_id).fetch_all(&pool).await?;
    Ok(Json(rows.into_iter().map(|(name, description, inherits, permissions)| RoleResponse { name, description, inherits, permissions }).collect()))
}

pub(crate) async fn create_role(
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
    identity: Identity,
    Json(body): Json<CreateRoleRequest>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt).await?;
    require_admin(&pool, &app_id, identity.user_id).await?;

    if body.name.is_empty() { return Err(ApiError::BadRequest("role name must not be empty".into())); }
    if body.name == "admin" { return Err(ApiError::BadRequest("cannot create a role named 'admin' (reserved)".into())); }

    if !body.inherits.is_empty() {
        let existing = sqlx::query_as("SELECT name, inherits FROM rootcx_system.rbac_roles WHERE app_id = $1")
            .bind(&app_id).fetch_all(&pool).await?;
        validate_hierarchy(existing, &body.name, &body.inherits)?;
    }

    let inherits: Vec<&str> = body.inherits.iter().map(|s| s.as_str()).collect();
    let permissions: Vec<&str> = body.permissions.iter().map(|s| s.as_str()).collect();

    sqlx::query("INSERT INTO rootcx_system.rbac_roles (app_id, name, description, inherits, permissions) VALUES ($1, $2, $3, $4, $5)")
        .bind(&app_id).bind(&body.name).bind(body.description.as_deref()).bind(&inherits).bind(&permissions)
        .execute(&pool).await
        .map_err(|e| match e {
            sqlx::Error::Database(ref db) if db.constraint().is_some() => ApiError::BadRequest(format!("role '{}' already exists", body.name)),
            _ => ApiError::Internal(e.to_string()),
        })?;

    Ok(Json(json!({ "message": format!("role '{}' created", body.name) })))
}

pub(crate) async fn update_role(
    State(rt): State<SharedRuntime>,
    Path((app_id, role_name)): Path<(String, String)>,
    identity: Identity,
    Json(body): Json<UpdateRoleRequest>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt).await?;
    require_admin(&pool, &app_id, identity.user_id).await?;

    if role_name == "admin" && (body.permissions.is_some() || body.inherits.is_some()) {
        return Err(ApiError::BadRequest("cannot modify permissions or inherits on built-in admin role".into()));
    }
    if body.description.is_none() && body.inherits.is_none() && body.permissions.is_none() {
        return Err(ApiError::BadRequest("no fields to update".into()));
    }

    if let Some(ref new_inherits) = body.inherits {
        let existing = sqlx::query_as("SELECT name, inherits FROM rootcx_system.rbac_roles WHERE app_id = $1")
            .bind(&app_id).fetch_all(&pool).await?;
        validate_hierarchy(existing, &role_name, new_inherits)?;
    }

    let mut qb: sqlx::QueryBuilder<sqlx::Postgres> = sqlx::QueryBuilder::new("UPDATE rootcx_system.rbac_roles SET ");
    let mut first = true;
    if let Some(ref d) = body.description {
        qb.push("description = ").push_bind(d.as_str());
        first = false;
    }
    if let Some(ref i) = body.inherits {
        if !first { qb.push(", "); }
        qb.push("inherits = ").push_bind(i.iter().map(|s| s.as_str()).collect::<Vec<_>>());
        first = false;
    }
    if let Some(ref p) = body.permissions {
        if !first { qb.push(", "); }
        qb.push("permissions = ").push_bind(p.iter().map(|s| s.as_str()).collect::<Vec<_>>());
    }
    qb.push(" WHERE app_id = ").push_bind(&app_id);
    qb.push(" AND name = ").push_bind(&role_name);

    let r = qb.build().execute(&pool).await?;
    if r.rows_affected() == 0 { return Err(ApiError::NotFound(format!("role '{role_name}' not found"))); }
    Ok(Json(json!({ "message": format!("role '{role_name}' updated") })))
}

pub(crate) async fn delete_role(
    State(rt): State<SharedRuntime>,
    Path((app_id, role_name)): Path<(String, String)>,
    identity: Identity,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt).await?;
    require_admin(&pool, &app_id, identity.user_id).await?;
    if role_name == "admin" { return Err(ApiError::BadRequest("cannot delete built-in admin role".into())); }

    let r = sqlx::query("DELETE FROM rootcx_system.rbac_roles WHERE app_id = $1 AND name = $2")
        .bind(&app_id).bind(&role_name).execute(&pool).await?;
    if r.rows_affected() == 0 { return Err(ApiError::NotFound(format!("role '{role_name}' not found"))); }

    sqlx::query("DELETE FROM rootcx_system.rbac_assignments WHERE app_id = $1 AND role = $2")
        .bind(&app_id).bind(&role_name).execute(&pool).await?;
    Ok(Json(json!({ "message": format!("role '{role_name}' deleted") })))
}

pub(crate) async fn list_assignments(
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
    identity: Identity,
) -> Result<Json<Vec<AssignmentResponse>>, ApiError> {
    let pool = routes::pool(&rt).await?;
    require_admin(&pool, &app_id, identity.user_id).await?;
    let rows: Vec<(Uuid, String, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT user_id, role, assigned_at FROM rootcx_system.rbac_assignments WHERE app_id = $1 ORDER BY assigned_at DESC",
    ).bind(&app_id).fetch_all(&pool).await?;
    Ok(Json(rows.into_iter().map(|(user_id, role, assigned_at)| AssignmentResponse { user_id, role, assigned_at }).collect()))
}

pub(crate) async fn assign_role(
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
    identity: Identity,
    Json(body): Json<RoleAssignment>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt).await?;
    require_admin(&pool, &app_id, identity.user_id).await?;

    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM rootcx_system.rbac_roles WHERE app_id = $1 AND name = $2)")
        .bind(&app_id).bind(&body.role).fetch_one(&pool).await?;
    if !exists { return Err(ApiError::BadRequest(format!("role '{}' not defined for app '{app_id}'", body.role))); }

    sqlx::query("INSERT INTO rootcx_system.rbac_assignments (user_id, app_id, role) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING")
        .bind(body.user_id).bind(&app_id).bind(&body.role).execute(&pool).await?;
    Ok(Json(json!({ "message": format!("role '{}' assigned", body.role) })))
}

pub(crate) async fn revoke_role(
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
    identity: Identity,
    Json(body): Json<RoleAssignment>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt).await?;
    require_admin(&pool, &app_id, identity.user_id).await?;

    let mut tx = pool.begin().await?;
    // Lock admin rows to prevent race on last-admin check
    let admin_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM (SELECT 1 FROM rootcx_system.rbac_assignments WHERE app_id = $1 AND role = 'admin' FOR UPDATE) t",
    ).bind(&app_id).fetch_one(&mut *tx).await?;

    if body.role == "admin" && admin_count <= 1 {
        return Err(ApiError::BadRequest("cannot revoke the last admin of an app".into()));
    }

    let r = sqlx::query("DELETE FROM rootcx_system.rbac_assignments WHERE user_id = $1 AND app_id = $2 AND role = $3")
        .bind(body.user_id).bind(&app_id).bind(&body.role).execute(&mut *tx).await?;
    tx.commit().await?;

    if r.rows_affected() == 0 { return Err(ApiError::NotFound(format!("assignment not found for role '{}'", body.role))); }
    Ok(Json(json!({ "message": format!("role '{}' revoked", body.role) })))
}

pub(crate) async fn my_permissions(
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
    identity: Identity,
) -> Result<Json<EffectivePermissions>, ApiError> {
    let pool = routes::pool(&rt).await?;
    let (roles, permissions) = resolve_permissions(&pool, &app_id, identity.user_id).await?;
    Ok(Json(EffectivePermissions { roles, permissions }))
}

pub(crate) async fn user_permissions(
    State(rt): State<SharedRuntime>,
    Path((app_id, target)): Path<(String, Uuid)>,
    identity: Identity,
) -> Result<Json<EffectivePermissions>, ApiError> {
    let pool = routes::pool(&rt).await?;
    if identity.user_id != target { require_admin(&pool, &app_id, identity.user_id).await?; }
    let (roles, permissions) = resolve_permissions(&pool, &app_id, target).await?;
    Ok(Json(EffectivePermissions { roles, permissions }))
}

pub(crate) async fn list_available_permissions(
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
) -> Result<Json<Vec<PermissionDeclarationResponse>>, ApiError> {
    let pool = routes::pool(&rt).await?;
    // App permissions + global tool permissions (stored under 'core')
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT key, description FROM rootcx_system.rbac_permissions
         WHERE app_id = $1 OR (app_id = 'core' AND key LIKE 'tool.%')
         ORDER BY key",
    ).bind(&app_id).fetch_all(&pool).await?;
    Ok(Json(rows.into_iter().map(|(key, description)| PermissionDeclarationResponse { key, description }).collect()))
}
