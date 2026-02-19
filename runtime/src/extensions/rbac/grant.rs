use std::sync::Arc;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::Method;
use uuid::Uuid;

use super::policy::PolicyCache;
use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::routes::{self, SharedRuntime};
use super::policy::{evaluate, resolve_user_roles};

pub struct AccessGrant {
    pub user_id: Option<Uuid>,
    pub ownership_required: bool,
}

#[cfg(test)]
pub(crate) fn test_grant(user_id: Option<Uuid>, ownership_required: bool) -> AccessGrant {
    AccessGrant { user_id, ownership_required }
}

fn method_to_action(method: &Method) -> &'static str {
    match *method {
        Method::GET => "read",
        Method::POST => "create",
        Method::PATCH | Method::PUT => "update",
        Method::DELETE => "delete",
        _ => "read",
    }
}

/// Parse /api/v1/apps/{app_id}/collections/{entity}[/{id}] from URI.
fn parse_crud_path(path: &str) -> Option<(&str, &str)> {
    let rest = path.strip_prefix("/api/v1/apps/")?;
    let mut segments = rest.split('/');
    let app_id = segments.next()?;
    if segments.next()? != "collections" { return None; }
    let entity = segments.next().filter(|e| !e.is_empty())?;
    Some((app_id, entity))
}

impl FromRequestParts<SharedRuntime> for AccessGrant {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &SharedRuntime,
    ) -> Result<Self, Self::Rejection> {
        let (app_id, entity) = parse_crud_path(parts.uri.path())
            .ok_or_else(|| ApiError::BadRequest("invalid CRUD path".into()))?;
        let app_id = app_id.to_string();
        let entity = entity.to_string();
        let method = parts.method.clone();

        let cache = parts.extensions.get::<Arc<PolicyCache>>().cloned()
            .ok_or_else(|| ApiError::Internal("rbac not configured".into()))?;

        let pool = routes::pool(state).await?;

        let cached = match cache.get_or_fetch(&pool, &app_id).await? {
            Some(c) => c,
            None => return Ok(AccessGrant { user_id: None, ownership_required: false }),
        };

        let identity = Identity::from_request_parts(parts, state).await?;
        let expanded = resolve_user_roles(&pool, &cached, identity.user_id, &app_id).await?;

        if expanded.is_empty() {
            return Err(ApiError::Forbidden(format!("no roles assigned for app '{app_id}'")));
        }

        let action = method_to_action(&method);
        let (allowed, ownership) = evaluate(&expanded, &entity, action, &cached.policies);

        if !allowed {
            return Err(ApiError::Forbidden(format!("action '{action}' on '{entity}' denied")));
        }

        Ok(AccessGrant { user_id: Some(identity.user_id), ownership_required: ownership })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Method;

    #[test]
    fn method_to_action_maps_crud_verbs() {
        for (method, expected) in [
            (Method::GET, "read"),
            (Method::POST, "create"),
            (Method::PATCH, "update"),
            (Method::PUT, "update"),
            (Method::DELETE, "delete"),
            (Method::OPTIONS, "read"),   // fallback
            (Method::HEAD, "read"),      // fallback
        ] {
            assert_eq!(method_to_action(&method), expected, "failed for {method}");
        }
    }

    #[test]
    fn parse_crud_path_valid() {
        assert_eq!(
            parse_crud_path("/api/v1/apps/crm/collections/deals"),
            Some(("crm", "deals")),
        );
    }

    #[test]
    fn parse_crud_path_with_record_id() {
        // The entity should still be "deals", the trailing id is ignored
        let (app, entity) = parse_crud_path("/api/v1/apps/crm/collections/deals/abc-123").unwrap();
        assert_eq!((app, entity), ("crm", "deals"));
    }

    #[test]
    fn parse_crud_path_rejects_invalid() {
        for (label, input) in [
            ("empty", ""),
            ("no prefix", "/apps/crm/collections/deals"),
            ("wrong segment", "/api/v1/apps/crm/roles/admin"),
            ("missing entity", "/api/v1/apps/crm/collections"),
            ("trailing slash", "/api/v1/apps/crm/collections/"),
        ] {
            assert!(parse_crud_path(input).is_none(), "expected None for {label}");
        }
    }
}
