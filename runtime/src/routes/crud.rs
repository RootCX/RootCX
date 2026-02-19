use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, NaiveDate, Utc};
use serde_json::Value as JsonValue;
use uuid::Uuid;

use crate::api_error::ApiError;
use crate::extensions::rbac::AccessGrant;
use crate::manifest::{field_type_map, map_field_type, quote_ident};
use super::{SharedRuntime, parse_uuid, pool};

fn table(app_id: &str, entity: &str) -> String {
    format!("{}.{}", quote_ident(app_id), quote_ident(entity))
}

fn require_object(body: &JsonValue) -> Result<&serde_json::Map<String, JsonValue>, ApiError> {
    let obj = body.as_object().ok_or_else(|| ApiError::BadRequest("body must be a JSON object".into()))?;
    if obj.is_empty() { return Err(ApiError::BadRequest("body must not be empty".into())); }
    Ok(obj)
}

fn owner_predicate(grant: &AccessGrant, param: usize, prefix: &str) -> Result<(String, Option<Uuid>), ApiError> {
    if grant.ownership_required {
        match grant.user_id {
            Some(uid) => Ok((format!("{prefix} owner_id = ${param}"), Some(uid))),
            None => Err(ApiError::Internal("ownership required but no user identity".into())),
        }
    } else {
        Ok((String::new(), None))
    }
}

pub async fn list_records(
    grant: AccessGrant,
    State(rt): State<SharedRuntime>,
    Path((app_id, entity)): Path<(String, String)>,
) -> Result<Json<Vec<JsonValue>>, ApiError> {
    let pool = pool(&rt).await?;
    let tbl = table(&app_id, &entity);
    let (filter, uid) = owner_predicate(&grant, 1, " WHERE")?;
    let q = format!("SELECT to_jsonb(t.*) AS row FROM {tbl} t{filter} ORDER BY created_at DESC");
    let mut query = sqlx::query_as::<_, (JsonValue,)>(&q);
    if let Some(uid) = uid { query = query.bind(uid); }
    let rows: Vec<(JsonValue,)> = query.fetch_all(&pool).await?;
    Ok(Json(rows.into_iter().map(|(r,)| r).collect()))
}

pub async fn create_record(
    grant: AccessGrant,
    State(rt): State<SharedRuntime>,
    Path((app_id, entity)): Path<(String, String)>,
    Json(body): Json<JsonValue>,
) -> Result<(StatusCode, Json<JsonValue>), ApiError> {
    let pool = pool(&rt).await?;
    let obj = require_object(&body)?;
    let tbl = table(&app_id, &entity);
    let types = field_type_map(&pool, &app_id, &entity).await?;

    let mut cols: Vec<String> = Vec::new();
    let mut phs: Vec<String> = Vec::new();
    let mut idx = 1usize;

    let owner_uid = if grant.ownership_required { grant.user_id } else { None };
    if owner_uid.is_some() {
        cols.push("\"owner_id\"".into());
        phs.push(format!("${idx}"));
        idx += 1;
    }
    for k in obj.keys() {
        cols.push(quote_ident(k));
        phs.push(format!("${idx}"));
        idx += 1;
    }

    let q = format!("INSERT INTO {tbl} ({}) VALUES ({}) RETURNING to_jsonb({tbl}.*) AS row", cols.join(", "), phs.join(", "));
    let mut query = sqlx::query_as::<_, (JsonValue,)>(&q);
    if let Some(uid) = owner_uid { query = query.bind(uid); }
    for (k, v) in obj.iter() { query = bind_typed(query, v, types.get(k.as_str())); }

    let (row,) = query.fetch_one(&pool).await?;
    Ok((StatusCode::CREATED, Json(row)))
}

pub async fn get_record(
    grant: AccessGrant,
    State(rt): State<SharedRuntime>,
    Path((app_id, entity, id)): Path<(String, String, String)>,
) -> Result<Json<JsonValue>, ApiError> {
    let (uuid, pool) = (parse_uuid(&id)?, pool(&rt).await?);
    let tbl = table(&app_id, &entity);
    let (owner, uid) = owner_predicate(&grant, 2, " AND")?;
    let q = format!("SELECT to_jsonb(t.*) AS row FROM {tbl} t WHERE t.id = $1{owner}");
    let mut query = sqlx::query_as::<_, (JsonValue,)>(&q).bind(uuid);
    if let Some(uid) = uid { query = query.bind(uid); }
    query.fetch_optional(&pool).await?
        .map(|(r,)| Json(r))
        .ok_or_else(|| ApiError::NotFound(format!("record '{id}' not found")))
}

pub async fn update_record(
    grant: AccessGrant,
    State(rt): State<SharedRuntime>,
    Path((app_id, entity, id)): Path<(String, String, String)>,
    Json(body): Json<JsonValue>,
) -> Result<Json<JsonValue>, ApiError> {
    let (uuid, pool) = (parse_uuid(&id)?, pool(&rt).await?);
    let obj = require_object(&body)?;
    let tbl = table(&app_id, &entity);
    let types = field_type_map(&pool, &app_id, &entity).await?;
    let entries: Vec<(&str, &JsonValue)> = obj.iter().map(|(k, v)| (k.as_str(), v)).collect();
    let sets: Vec<String> = entries.iter().enumerate()
        .map(|(i, (k, _))| format!("{} = ${}", quote_ident(k), i + 1)).collect();
    let id_param = entries.len() + 1;
    let (owner, uid) = owner_predicate(&grant, id_param + 1, " AND")?;
    let q = format!("UPDATE {tbl} SET {}, \"updated_at\" = now() WHERE id = ${id_param}{owner} RETURNING to_jsonb({tbl}.*) AS row", sets.join(", "));
    let mut query = sqlx::query_as::<_, (JsonValue,)>(&q);
    for (k, v) in &entries { query = bind_typed(query, v, types.get(*k)); }
    query = query.bind(uuid);
    if let Some(uid) = uid { query = query.bind(uid); }
    query.fetch_optional(&pool).await?
        .map(|(r,)| Json(r))
        .ok_or_else(|| ApiError::NotFound(format!("record '{id}' not found")))
}

pub async fn delete_record(
    grant: AccessGrant,
    State(rt): State<SharedRuntime>,
    Path((app_id, entity, id)): Path<(String, String, String)>,
) -> Result<Json<JsonValue>, ApiError> {
    let (uuid, pool) = (parse_uuid(&id)?, pool(&rt).await?);
    let tbl = table(&app_id, &entity);
    let (owner, uid) = owner_predicate(&grant, 2, " AND")?;
    let q = format!("DELETE FROM {tbl} WHERE id = $1{owner}");
    let mut query = sqlx::query(&q).bind(uuid);
    if let Some(uid) = uid { query = query.bind(uid); }
    let r = query.execute(&pool).await?;
    if r.rows_affected() == 0 { return Err(ApiError::NotFound(format!("record '{id}' not found"))); }
    Ok(Json(serde_json::json!({ "message": format!("record '{id}' deleted") })))
}

type QA<'q> = sqlx::query::QueryAs<'q, sqlx::Postgres, (JsonValue,), sqlx::postgres::PgArguments>;

/// Bind a JSON value as the native PG type derived from `map_field_type`.
fn bind_typed<'q>(q: QA<'q>, val: &'q JsonValue, manifest_type: Option<&String>) -> QA<'q> {
    let pg = manifest_type.map(|t| map_field_type(t));
    match val {
        JsonValue::Null => q.bind(None::<String>),
        JsonValue::Bool(b) => q.bind(b),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() { q.bind(i) } else { q.bind(n.as_f64().unwrap_or(0.0)) }
        }
        JsonValue::String(s) => match pg {
            Some("UUID") => q.bind(s.parse::<Uuid>().ok()),
            Some("DATE") => q.bind(s.parse::<NaiveDate>().ok()),
            Some("TIMESTAMPTZ") => q.bind(s.parse::<DateTime<Utc>>().ok()),
            _ => q.bind(s.as_str()),
        },
        JsonValue::Array(arr) => match pg {
            Some("TEXT[]") => q.bind(arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>()),
            Some("DOUBLE PRECISION[]") => q.bind(arr.iter().filter_map(|v| v.as_f64()).collect::<Vec<_>>()),
            _ => q.bind(val),
        },
        _ => q.bind(val),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensions::rbac::grant::test_grant;
    use serde_json::json;

    #[test]
    fn table_sanitizes_inputs() {
        let result = table("my;app", "drop--table");
        assert!(!result.contains(';') && !result.contains('-'));
        assert_eq!(result, "\"myapp\".\"droptable\"");
    }

    #[test]
    fn require_object_valid() { assert!(require_object(&json!({"a": 1})).is_ok()); }

    #[test]
    fn require_object_rejects_invalid() {
        for (label, input) in [("empty", json!({})), ("array", json!([1, 2])), ("string", json!("hello")), ("null", json!(null))] {
            assert!(require_object(&input).is_err(), "expected error for {label}");
        }
    }

    #[test]
    fn owner_predicate_with_prefix() {
        for (param, prefix, expected) in [
            (3, " AND", " AND owner_id = $3"),
            (1, " WHERE", " WHERE owner_id = $1"),
        ] {
            let uid = Uuid::new_v4();
            let grant = test_grant(Some(uid), true);
            let (clause, bound) = owner_predicate(&grant, param, prefix).unwrap();
            assert_eq!(clause, expected, "prefix={prefix:?}");
            assert_eq!(bound, Some(uid));
        }
    }

    #[test]
    fn owner_predicate_without_ownership() {
        let grant = test_grant(Some(Uuid::new_v4()), false);
        let (clause, bound) = owner_predicate(&grant, 1, " AND").unwrap();
        assert!(clause.is_empty());
        assert!(bound.is_none());
    }

    #[test]
    fn owner_predicate_errors_on_impossible_state() {
        let grant = test_grant(None, true);
        assert!(owner_predicate(&grant, 1, " AND").is_err());
    }
}
