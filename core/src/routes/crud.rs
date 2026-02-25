use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use chrono::{DateTime, NaiveDate, Utc};
use serde::Deserialize;
use serde_json::Value as JsonValue;
use uuid::Uuid;

use super::{SharedRuntime, parse_uuid, pool};
use crate::api_error::ApiError;
use crate::extensions::rbac::AccessGrant;
use crate::manifest::{field_type_map, map_field_type, quote_ident};

pub(crate) fn table(app_id: &str, entity: &str) -> String {
    format!("{}.{}", quote_ident(app_id), quote_ident(entity))
}

fn require_object(body: &JsonValue) -> Result<&serde_json::Map<String, JsonValue>, ApiError> {
    let obj = body.as_object().ok_or_else(|| ApiError::BadRequest("body must be a JSON object".into()))?;
    if obj.is_empty() {
        return Err(ApiError::BadRequest("body must not be empty".into()));
    }
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

const RESERVED_PARAMS: &[&str] = &["limit", "offset", "sort", "order"];

fn pg_cast_suffix(manifest_type: Option<&str>) -> &'static str {
    match manifest_type.map(map_field_type) {
        Some("DOUBLE PRECISION") => "::float8",
        Some("BOOLEAN") => "::boolean",
        Some("DATE") => "::date",
        Some("TIMESTAMPTZ") => "::timestamptz",
        Some("UUID") => "::uuid",
        Some("JSONB") => "::jsonb",
        Some("TEXT[]") => "::text[]",
        Some("DOUBLE PRECISION[]") => "::float8[]",
        _ => "",
    }
}

fn json_value_to_string(val: &JsonValue) -> String {
    match val {
        JsonValue::String(s) => s.clone(),
        JsonValue::Number(n) => n.to_string(),
        JsonValue::Bool(b) => b.to_string(),
        JsonValue::Null => String::new(),
        _ => val.to_string(),
    }
}

pub(crate) fn validate_sort_field(field: Option<&String>, types: &HashMap<String, String>) -> String {
    match field {
        Some(f)
            if types.contains_key(f.as_str())
                || matches!(f.as_str(), "created_at" | "updated_at" | "id") =>
        {
            quote_ident(f)
        }
        _ => "\"created_at\"".to_string(),
    }
}

pub(crate) fn validate_order(s: Option<&String>) -> &'static str {
    match s.map(String::as_str) {
        Some("asc" | "ASC") => "ASC",
        _ => "DESC",
    }
}

pub(crate) fn build_where_clause(
    clause: &JsonValue,
    types: &HashMap<String, String>,
    binds: &mut Vec<String>,
    idx: &mut usize,
) -> Result<String, ApiError> {
    let obj = clause
        .as_object()
        .ok_or_else(|| ApiError::BadRequest("where clause must be a JSON object".into()))?;

    let mut parts: Vec<String> = Vec::new();

    for (key, val) in obj {
        match key.as_str() {
            "$and" | "$or" => parts.push(build_logical(key, val, types, binds, idx)?),
            "$not" => {
                let sub = build_where_clause(val, types, binds, idx)?;
                parts.push(format!("NOT ({sub})"));
            }
            field => parts.push(build_field_condition(field, val, types, binds, idx)?),
        }
    }

    if parts.is_empty() { Ok("TRUE".into()) } else { Ok(parts.join(" AND ")) }
}

fn build_logical(
    op: &str,
    val: &JsonValue,
    types: &HashMap<String, String>,
    binds: &mut Vec<String>,
    idx: &mut usize,
) -> Result<String, ApiError> {
    let sep = if op == "$and" { " AND " } else { " OR " };
    let arr = val
        .as_array()
        .ok_or_else(|| ApiError::BadRequest(format!("{op} must be an array")))?;
    let subs: Vec<String> = arr
        .iter()
        .map(|s| build_where_clause(s, types, binds, idx))
        .collect::<Result<_, _>>()?;
    if subs.is_empty() { Ok("TRUE".into()) } else { Ok(format!("({})", subs.join(sep))) }
}

fn push_ownership(
    grant: &AccessGrant,
    conditions: &mut Vec<String>,
    binds: &mut Vec<String>,
    idx: &mut usize,
) -> Result<(), ApiError> {
    if grant.ownership_required {
        let uid = grant
            .user_id
            .ok_or_else(|| ApiError::Internal("ownership required but no user identity".into()))?;
        *idx += 1;
        binds.push(uid.to_string());
        conditions.push(format!("owner_id = ${}::uuid", *idx));
    }
    Ok(())
}

pub(crate) fn join_where(conditions: &[String]) -> String {
    if conditions.is_empty() { String::new() } else { format!(" WHERE {}", conditions.join(" AND ")) }
}

fn build_field_condition(
    field: &str,
    val: &JsonValue,
    types: &HashMap<String, String>,
    binds: &mut Vec<String>,
    idx: &mut usize,
) -> Result<String, ApiError> {
    let col = quote_ident(field);
    let cast = pg_cast_suffix(types.get(field).map(String::as_str));

    if !val.is_object() {
        if val.is_null() { return Ok(format!("{col} IS NULL")); }
        *idx += 1;
        binds.push(json_value_to_string(val));
        return Ok(format!("{col} = ${}{cast}", *idx));
    }

    let ops = val.as_object().unwrap();
    if !ops.keys().any(|k| k.starts_with('$')) {
        *idx += 1;
        binds.push(val.to_string());
        return Ok(format!("{col} = ${}::jsonb", *idx));
    }

    let mut conditions: Vec<String> = Vec::new();

    for (op, operand) in ops {
        match op.as_str() {
            "$eq" => {
                if operand.is_null() {
                    conditions.push(format!("{col} IS NULL"));
                } else {
                    *idx += 1;
                    binds.push(json_value_to_string(operand));
                    conditions.push(format!("{col} = ${}{cast}", *idx));
                }
            }
            "$ne" => {
                if operand.is_null() {
                    conditions.push(format!("{col} IS NOT NULL"));
                } else {
                    *idx += 1;
                    binds.push(json_value_to_string(operand));
                    conditions.push(format!("{col} != ${}{cast}", *idx));
                }
            }
            "$gt" | "$gte" | "$lt" | "$lte" => {
                let sql_op = match op.as_str() {
                    "$gt" => ">",
                    "$gte" => ">=",
                    "$lt" => "<",
                    _ => "<=",
                };
                *idx += 1;
                binds.push(json_value_to_string(operand));
                conditions.push(format!("{col} {sql_op} ${}{cast}", *idx));
            }
            "$like" | "$ilike" => {
                let kw = if op == "$like" { "LIKE" } else { "ILIKE" };
                *idx += 1;
                binds.push(json_value_to_string(operand));
                conditions.push(format!("{col} {kw} ${}", *idx));
            }
            "$in" => {
                let arr = operand
                    .as_array()
                    .ok_or_else(|| ApiError::BadRequest("$in must be an array".into()))?;
                if arr.is_empty() {
                    conditions.push("FALSE".into());
                } else {
                    let phs: Vec<String> = arr
                        .iter()
                        .map(|v| {
                            *idx += 1;
                            binds.push(json_value_to_string(v));
                            format!("${}{cast}", *idx)
                        })
                        .collect();
                    conditions.push(format!("{col} IN ({})", phs.join(", ")));
                }
            }
            "$contains" => {
                let arr = operand
                    .as_array()
                    .ok_or_else(|| ApiError::BadRequest("$contains must be an array".into()))?;
                if !arr.is_empty() {
                    let phs: Vec<String> = arr
                        .iter()
                        .map(|v| {
                            *idx += 1;
                            binds.push(json_value_to_string(v));
                            format!("${}", *idx)
                        })
                        .collect();
                    conditions.push(format!("{col} @> ARRAY[{}]{cast}", phs.join(", ")));
                }
            }
            "$isNull" => {
                let is_null = operand.as_bool().unwrap_or(true);
                conditions.push(if is_null {
                    format!("{col} IS NULL")
                } else {
                    format!("{col} IS NOT NULL")
                });
            }
            other => {
                return Err(ApiError::BadRequest(format!("unknown operator: '{other}'")));
            }
        }
    }

    Ok(conditions.join(" AND "))
}

#[derive(Deserialize)]
pub struct QueryRequest {
    #[serde(rename = "where", default)]
    where_clause: Option<JsonValue>,
    #[serde(rename = "orderBy")]
    order_by: Option<String>,
    #[serde(default)]
    order: Option<String>,
    #[serde(default = "default_query_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}

fn default_query_limit() -> i64 {
    100
}

pub async fn list_records(
    grant: AccessGrant,
    State(rt): State<SharedRuntime>,
    Path((app_id, entity)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Vec<JsonValue>>, ApiError> {
    let pool = pool(&rt).await?;
    let tbl = table(&app_id, &entity);
    let types = field_type_map(&pool, &app_id, &entity).await?;

    let (mut idx, mut conditions, mut binds) = (0usize, Vec::new(), Vec::new());
    push_ownership(&grant, &mut conditions, &mut binds, &mut idx)?;

    for (key, val) in &params {
        if RESERVED_PARAMS.contains(&key.as_str()) { continue; }
        let cast = pg_cast_suffix(types.get(key.as_str()).map(String::as_str));
        idx += 1;
        binds.push(val.clone());
        conditions.push(format!("{} = ${}{}", quote_ident(key), idx, cast));
    }

    let wh = join_where(&conditions);
    let sort = validate_sort_field(params.get("sort"), &types);
    let order = validate_order(params.get("order"));
    let limit = params.get("limit").map(|l| format!(" LIMIT {}", l.parse::<i64>().unwrap_or(100).min(1000).max(1))).unwrap_or_default();
    let offset = params.get("offset").map(|o| format!(" OFFSET {}", o.parse::<i64>().unwrap_or(0).max(0))).unwrap_or_default();

    let q = format!("SELECT to_jsonb(t.*) AS row FROM {tbl} t{wh} ORDER BY {sort} {order}{limit}{offset}");
    let mut query = sqlx::query_as::<_, (JsonValue,)>(&q);
    for s in &binds { query = query.bind(s.as_str()); }
    let rows: Vec<(JsonValue,)> = query.fetch_all(&pool).await?;
    Ok(Json(rows.into_iter().map(|(r,)| r).collect()))
}

pub async fn query_records(
    grant: AccessGrant,
    State(rt): State<SharedRuntime>,
    Path((app_id, entity)): Path<(String, String)>,
    Json(body): Json<QueryRequest>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = pool(&rt).await?;
    let tbl = table(&app_id, &entity);
    let types = field_type_map(&pool, &app_id, &entity).await?;

    let (mut idx, mut conditions, mut binds) = (0usize, Vec::new(), Vec::new());
    push_ownership(&grant, &mut conditions, &mut binds, &mut idx)?;

    if let Some(ref w) = body.where_clause {
        let sql = build_where_clause(w, &types, &mut binds, &mut idx)?;
        if sql != "TRUE" { conditions.push(sql); }
    }

    let wh = join_where(&conditions);
    let sort = validate_sort_field(body.order_by.as_ref(), &types);
    let order = validate_order(body.order.as_ref());
    let limit = body.limit.min(1000).max(1);
    let offset = body.offset.max(0);

    let q = format!(
        "SELECT to_jsonb(t.*) AS row, COUNT(*) OVER() AS total \
         FROM {tbl} t{wh} ORDER BY {sort} {order} LIMIT {limit} OFFSET {offset}"
    );
    let mut query = sqlx::query_as::<_, (JsonValue, i64)>(&q);
    for s in &binds { query = query.bind(s.as_str()); }
    let rows: Vec<(JsonValue, i64)> = query.fetch_all(&pool).await?;

    let total = rows.first().map(|(_, t)| *t).unwrap_or(0);
    let data: Vec<JsonValue> = rows.into_iter().map(|(r, _)| r).collect();
    Ok(Json(serde_json::json!({ "data": data, "total": total })))
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

    let q = format!(
        "INSERT INTO {tbl} ({}) VALUES ({}) RETURNING to_jsonb({tbl}.*) AS row",
        cols.join(", "),
        phs.join(", ")
    );
    let mut query = sqlx::query_as::<_, (JsonValue,)>(&q);
    if let Some(uid) = owner_uid {
        query = query.bind(uid);
    }
    for (k, v) in obj.iter() {
        query = bind_typed(query, v, types.get(k.as_str()));
    }

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
    if let Some(uid) = uid {
        query = query.bind(uid);
    }
    query
        .fetch_optional(&pool)
        .await?
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
    let sets: Vec<String> =
        entries.iter().enumerate().map(|(i, (k, _))| format!("{} = ${}", quote_ident(k), i + 1)).collect();
    let id_param = entries.len() + 1;
    let (owner, uid) = owner_predicate(&grant, id_param + 1, " AND")?;
    let q = format!(
        "UPDATE {tbl} SET {}, \"updated_at\" = now() WHERE id = ${id_param}{owner} RETURNING to_jsonb({tbl}.*) AS row",
        sets.join(", ")
    );
    let mut query = sqlx::query_as::<_, (JsonValue,)>(&q);
    for (k, v) in &entries {
        query = bind_typed(query, v, types.get(*k));
    }
    query = query.bind(uuid);
    if let Some(uid) = uid {
        query = query.bind(uid);
    }
    query
        .fetch_optional(&pool)
        .await?
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
    if let Some(uid) = uid {
        query = query.bind(uid);
    }
    let r = query.execute(&pool).await?;
    if r.rows_affected() == 0 {
        return Err(ApiError::NotFound(format!("record '{id}' not found")));
    }
    Ok(Json(serde_json::json!({ "message": format!("record '{id}' deleted") })))
}

pub(crate) type QA<'q> = sqlx::query::QueryAs<'q, sqlx::Postgres, (JsonValue,), sqlx::postgres::PgArguments>;

pub(crate) fn bind_typed<'q>(q: QA<'q>, val: &'q JsonValue, manifest_type: Option<&String>) -> QA<'q> {
    let pg = manifest_type.map(|t| map_field_type(t));
    match val {
        JsonValue::Null => match pg {
            Some("UUID") => q.bind(None::<Uuid>),
            Some("DATE") => q.bind(None::<NaiveDate>),
            Some("TIMESTAMPTZ") => q.bind(None::<DateTime<Utc>>),
            Some("BOOLEAN") => q.bind(None::<bool>),
            Some("DOUBLE PRECISION") => q.bind(None::<f64>),
            Some("JSONB") => q.bind(None::<JsonValue>),
            Some("TEXT[]") => q.bind(None::<Vec<String>>),
            Some("DOUBLE PRECISION[]") => q.bind(None::<Vec<f64>>),
            _ => q.bind(None::<String>),
        },
        JsonValue::Bool(b) => q.bind(b),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                q.bind(i)
            } else {
                q.bind(n.as_f64().unwrap_or(0.0))
            }
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
    fn table_quotes_identifiers() {
        assert_eq!(table("my_app", "contacts"), "\"my_app\".\"contacts\"");
    }

    #[test]
    fn require_object_valid() {
        assert!(require_object(&json!({"a": 1})).is_ok());
    }

    #[test]
    fn require_object_rejects_invalid() {
        for (label, input) in
            [("empty", json!({})), ("array", json!([1, 2])), ("string", json!("hello")), ("null", json!(null))]
        {
            assert!(require_object(&input).is_err(), "expected error for {label}");
        }
    }

    #[test]
    fn owner_predicate_with_prefix() {
        for (param, prefix, expected) in [(3, " AND", " AND owner_id = $3"), (1, " WHERE", " WHERE owner_id = $1")] {
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

    fn types_fixture() -> HashMap<String, String> {
        [
            ("status", "text"),
            ("age", "number"),
            ("active", "boolean"),
            ("tags", "[text]"),
            ("score", "number"),
            ("created", "timestamp"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
    }

    #[test]
    fn where_simple_equality() {
        let types = types_fixture();
        let mut binds = Vec::new();
        let mut idx = 0;
        let sql = build_where_clause(&json!({"status": "active"}), &types, &mut binds, &mut idx).unwrap();
        assert_eq!(sql, "\"status\" = $1");
        assert_eq!(binds, vec!["active"]);
    }

    #[test]
    fn where_null_shorthand() {
        let types = types_fixture();
        let mut binds = Vec::new();
        let mut idx = 0;
        let sql = build_where_clause(&json!({"status": null}), &types, &mut binds, &mut idx).unwrap();
        assert_eq!(sql, "\"status\" IS NULL");
        assert!(binds.is_empty());
    }

    #[test]
    fn where_operator_gt() {
        let types = types_fixture();
        let mut binds = Vec::new();
        let mut idx = 0;
        let sql =
            build_where_clause(&json!({"age": {"$gt": 18}}), &types, &mut binds, &mut idx).unwrap();
        assert_eq!(sql, "\"age\" > $1::float8");
        assert_eq!(binds, vec!["18"]);
    }

    #[test]
    fn where_combined_operators() {
        let types = types_fixture();
        let mut binds = Vec::new();
        let mut idx = 0;
        let sql = build_where_clause(
            &json!({"age": {"$gte": 18, "$lt": 65}}),
            &types,
            &mut binds,
            &mut idx,
        )
        .unwrap();
        assert!(sql.contains("\"age\" >= $") && sql.contains("\"age\" < $"));
        assert_eq!(binds.len(), 2);
    }

    #[test]
    fn where_in_operator() {
        let types = types_fixture();
        let mut binds = Vec::new();
        let mut idx = 0;
        let sql = build_where_clause(
            &json!({"status": {"$in": ["active", "pending"]}}),
            &types,
            &mut binds,
            &mut idx,
        )
        .unwrap();
        assert_eq!(sql, "\"status\" IN ($1, $2)");
        assert_eq!(binds, vec!["active", "pending"]);
    }

    #[test]
    fn where_in_empty_is_false() {
        let types = types_fixture();
        let mut binds = Vec::new();
        let mut idx = 0;
        let sql =
            build_where_clause(&json!({"status": {"$in": []}}), &types, &mut binds, &mut idx)
                .unwrap();
        assert_eq!(sql, "FALSE");
    }

    #[test]
    fn where_is_null_operator() {
        let types = types_fixture();
        let mut binds = Vec::new();
        let mut idx = 0;
        let sql = build_where_clause(
            &json!({"status": {"$isNull": true}}),
            &types,
            &mut binds,
            &mut idx,
        )
        .unwrap();
        assert_eq!(sql, "\"status\" IS NULL");
        assert!(binds.is_empty());
    }

    #[test]
    fn where_or_combinator() {
        let types = types_fixture();
        let mut binds = Vec::new();
        let mut idx = 0;
        let sql = build_where_clause(
            &json!({"$or": [{"status": "active"}, {"status": "pending"}]}),
            &types,
            &mut binds,
            &mut idx,
        )
        .unwrap();
        assert_eq!(sql, "(\"status\" = $1 OR \"status\" = $2)");
        assert_eq!(binds, vec!["active", "pending"]);
    }

    #[test]
    fn where_and_combinator() {
        let types = types_fixture();
        let mut binds = Vec::new();
        let mut idx = 0;
        let sql = build_where_clause(
            &json!({"$and": [{"age": {"$gt": 18}}, {"age": {"$lt": 65}}]}),
            &types,
            &mut binds,
            &mut idx,
        )
        .unwrap();
        assert_eq!(sql, "(\"age\" > $1::float8 AND \"age\" < $2::float8)");
    }

    #[test]
    fn where_not_combinator() {
        let types = types_fixture();
        let mut binds = Vec::new();
        let mut idx = 0;
        let sql = build_where_clause(
            &json!({"$not": {"status": "deleted"}}),
            &types,
            &mut binds,
            &mut idx,
        )
        .unwrap();
        assert_eq!(sql, "NOT (\"status\" = $1)");
        assert_eq!(binds, vec!["deleted"]);
    }

    #[test]
    fn where_nested_or_and() {
        let types = types_fixture();
        let mut binds = Vec::new();
        let mut idx = 0;
        let clause = json!({
            "$and": [
                {"$or": [{"status": "active"}, {"status": "pending"}]},
                {"$or": [{"age": {"$gt": 18}}, {"active": true}]}
            ]
        });
        let sql = build_where_clause(&clause, &types, &mut binds, &mut idx).unwrap();
        assert!(sql.starts_with("((\"status\" = $1 OR \"status\" = $2) AND ("));
        assert_eq!(binds.len(), 4);
    }

    #[test]
    fn where_mixed_fields_and_or() {
        let types = types_fixture();
        let mut binds = Vec::new();
        let mut idx = 0;
        let clause = json!({
            "status": "active",
            "$or": [{"age": {"$gt": 18}}, {"age": {"$lt": 5}}]
        });
        let sql = build_where_clause(&clause, &types, &mut binds, &mut idx).unwrap();
        assert!(sql.contains("\"status\" = $"));
        assert!(sql.contains(" OR "));
    }

    #[test]
    fn where_like_no_cast() {
        let types = types_fixture();
        let mut binds = Vec::new();
        let mut idx = 0;
        let sql = build_where_clause(
            &json!({"status": {"$like": "%act%"}}),
            &types,
            &mut binds,
            &mut idx,
        )
        .unwrap();
        assert_eq!(sql, "\"status\" LIKE $1");
        assert_eq!(binds, vec!["%act%"]);
    }

    #[test]
    fn where_contains_array() {
        let types = types_fixture();
        let mut binds = Vec::new();
        let mut idx = 0;
        let sql = build_where_clause(
            &json!({"tags": {"$contains": ["vip"]}}),
            &types,
            &mut binds,
            &mut idx,
        )
        .unwrap();
        assert_eq!(sql, "\"tags\" @> ARRAY[$1]::text[]");
        assert_eq!(binds, vec!["vip"]);
    }

    #[test]
    fn where_unknown_operator_rejected() {
        let types = types_fixture();
        let mut binds = Vec::new();
        let mut idx = 0;
        let result = build_where_clause(
            &json!({"status": {"$regex": ".*"}}),
            &types,
            &mut binds,
            &mut idx,
        );
        assert!(result.is_err());
    }

    #[test]
    fn where_empty_clause_is_true() {
        let types = types_fixture();
        let mut binds = Vec::new();
        let mut idx = 0;
        let sql = build_where_clause(&json!({}), &types, &mut binds, &mut idx).unwrap();
        assert_eq!(sql, "TRUE");
    }

    #[test]
    fn pg_cast_suffix_covers_types() {
        assert_eq!(pg_cast_suffix(Some("number")), "::float8");
        assert_eq!(pg_cast_suffix(Some("boolean")), "::boolean");
        assert_eq!(pg_cast_suffix(Some("date")), "::date");
        assert_eq!(pg_cast_suffix(Some("timestamp")), "::timestamptz");
        assert_eq!(pg_cast_suffix(Some("entity_link")), "::uuid");
        assert_eq!(pg_cast_suffix(Some("text")), "");
        assert_eq!(pg_cast_suffix(None), "");
    }

    #[test]
    fn validate_sort_accepts_known_fields() {
        let types = types_fixture();
        assert_eq!(validate_sort_field(Some(&"status".into()), &types), "\"status\"");
        assert_eq!(validate_sort_field(Some(&"created_at".into()), &types), "\"created_at\"");
        assert_eq!(validate_sort_field(Some(&"unknown_field".into()), &types), "\"created_at\"");
        assert_eq!(validate_sort_field(None, &types), "\"created_at\"");
    }
}
