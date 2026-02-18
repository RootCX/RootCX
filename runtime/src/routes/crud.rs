use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde_json::Value as JsonValue;

use crate::api_error::ApiError;
use crate::manifest::quote_ident;
use super::{SharedRuntime, parse_uuid, pool};

fn table(app_id: &str, entity: &str) -> String {
    format!("{}.{}", quote_ident(app_id), quote_ident(entity))
}

fn require_object(body: &JsonValue) -> Result<&serde_json::Map<String, JsonValue>, ApiError> {
    let obj = body.as_object().ok_or_else(|| ApiError::BadRequest("body must be a JSON object".into()))?;
    if obj.is_empty() { return Err(ApiError::BadRequest("body must not be empty".into())); }
    Ok(obj)
}

pub async fn list_records(State(rt): State<SharedRuntime>, Path((app_id, entity)): Path<(String, String)>) -> Result<Json<Vec<JsonValue>>, ApiError> {
    let pool = pool(&rt).await?;
    let q = format!("SELECT to_jsonb(t.*) AS row FROM {} t ORDER BY created_at DESC", table(&app_id, &entity));
    let rows: Vec<(JsonValue,)> = sqlx::query_as(&q).fetch_all(&pool).await?;
    Ok(Json(rows.into_iter().map(|(r,)| r).collect()))
}

pub async fn create_record(State(rt): State<SharedRuntime>, Path((app_id, entity)): Path<(String, String)>, Json(body): Json<JsonValue>) -> Result<(StatusCode, Json<JsonValue>), ApiError> {
    let pool = pool(&rt).await?;
    let obj = require_object(&body)?;
    let tbl = table(&app_id, &entity);
    let values: Vec<_> = obj.values().cloned().collect();
    let (cols, phs): (Vec<_>, Vec<_>) = obj.keys().enumerate().map(|(i, k)| (quote_ident(k), format!("${}", i + 1))).unzip();
    let q = format!("INSERT INTO {} ({}) VALUES ({}) RETURNING to_jsonb({}.*) AS row", tbl, cols.join(", "), phs.join(", "), tbl);
    let mut query = sqlx::query_as::<_, (JsonValue,)>(&q);
    for v in &values { query = bind_json_value(query, v); }
    let (row,) = query.fetch_one(&pool).await?;
    Ok((StatusCode::CREATED, Json(row)))
}

pub async fn get_record(State(rt): State<SharedRuntime>, Path((app_id, entity, id)): Path<(String, String, String)>) -> Result<Json<JsonValue>, ApiError> {
    let (uuid, pool) = (parse_uuid(&id)?, pool(&rt).await?);
    let q = format!("SELECT to_jsonb(t.*) AS row FROM {} t WHERE t.id = $1", table(&app_id, &entity));
    sqlx::query_as::<_, (JsonValue,)>(&q).bind(uuid).fetch_optional(&pool).await?
        .map(|(r,)| Json(r)).ok_or_else(|| ApiError::NotFound(format!("record '{id}' not found")))
}

pub async fn update_record(State(rt): State<SharedRuntime>, Path((app_id, entity, id)): Path<(String, String, String)>, Json(body): Json<JsonValue>) -> Result<Json<JsonValue>, ApiError> {
    let (uuid, pool) = (parse_uuid(&id)?, pool(&rt).await?);
    let obj = require_object(&body)?;
    let tbl = table(&app_id, &entity);
    let (sets, vals): (Vec<_>, Vec<_>) = obj.iter().enumerate().map(|(i, (k, v))| (format!("{} = ${}", quote_ident(k), i + 1), v.clone())).unzip();
    let q = format!("UPDATE {} SET {}, \"updated_at\" = now() WHERE id = ${} RETURNING to_jsonb({} .*) AS row", tbl, sets.join(", "), vals.len() + 1, tbl);
    let mut query = sqlx::query_as::<_, (JsonValue,)>(&q);
    for v in &vals { query = bind_json_value(query, v); }
    query = query.bind(uuid);
    query.fetch_optional(&pool).await?.map(|(r,)| Json(r)).ok_or_else(|| ApiError::NotFound(format!("record '{id}' not found")))
}

pub async fn delete_record(State(rt): State<SharedRuntime>, Path((app_id, entity, id)): Path<(String, String, String)>) -> Result<Json<JsonValue>, ApiError> {
    let (uuid, pool) = (parse_uuid(&id)?, pool(&rt).await?);
    let r = sqlx::query(&format!("DELETE FROM {} WHERE id = $1", table(&app_id, &entity))).bind(uuid).execute(&pool).await?;
    if r.rows_affected() == 0 { return Err(ApiError::NotFound(format!("record '{id}' not found"))); }
    Ok(Json(serde_json::json!({ "message": format!("record '{id}' deleted") })))
}

fn bind_json_value<'q>(
    q: sqlx::query::QueryAs<'q, sqlx::Postgres, (JsonValue,), sqlx::postgres::PgArguments>,
    val: &'q JsonValue,
) -> sqlx::query::QueryAs<'q, sqlx::Postgres, (JsonValue,), sqlx::postgres::PgArguments> {
    match val {
        JsonValue::Null => q.bind(None::<String>),
        JsonValue::Bool(b) => q.bind(b),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() { q.bind(i) } else { q.bind(n.as_f64().unwrap_or(0.0)) }
        }
        JsonValue::String(s) => q.bind(s.as_str()),
        _ => q.bind(val),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn table_formats_qualified_name() {
        assert_eq!(table("myapp", "contacts"), "\"myapp\".\"contacts\"");
    }

    #[test]
    fn table_sanitizes_inputs() {
        let result = table("my;app", "drop--table");
        assert!(!result.contains(';'));
        assert!(!result.contains('-'));
        assert_eq!(result, "\"myapp\".\"droptable\"");
    }

    #[test]
    fn require_object_valid() {
        assert!(require_object(&json!({"a": 1})).is_ok());
    }

    #[test]
    fn require_object_rejects_invalid() {
        for (label, input) in [
            ("empty", json!({})),
            ("array", json!([1, 2])),
            ("string", json!("hello")),
            ("null", json!(null)),
        ] {
            assert!(require_object(&input).is_err(), "expected error for {label}");
        }
    }
}
