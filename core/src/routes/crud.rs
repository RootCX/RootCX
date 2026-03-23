use std::collections::{HashMap, HashSet};

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use chrono::{DateTime, NaiveDate, Utc};
use serde::Deserialize;
use serde_json::Value as JsonValue;
use sqlx::PgPool;
use uuid::Uuid;

use super::{SharedRuntime, parse_uuid, pool};
use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::extensions::rbac::policy::resolve_permissions;
use crate::manifest::{entity_identity, field_type_map, find_entities_by_identity, map_field_type, quote_ident};

async fn require_perm(pool: &PgPool, app_id: &str, user_id: Uuid, perm: &str) -> Result<(), ApiError> {
    let (_, perms) = resolve_permissions(pool, app_id, user_id).await?;
    if perms.iter().any(|p| p == "*" || p == perm) { Ok(()) }
    else { Err(ApiError::Forbidden(format!("permission denied: {perm}"))) }
}

pub(crate) const MAX_BULK_SIZE: usize = 1000;
const PG_PARAM_LIMIT: usize = 65535;

pub(crate) fn validate_app_id(app_id: &str) -> Result<(), ApiError> {
    if matches!(app_id, "rootcx_system" | "pg_catalog" | "information_schema") || app_id.starts_with("pg_") {
        return Err(ApiError::Forbidden(format!("access to schema '{app_id}' is blocked")));
    }
    Ok(())
}

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

const RESERVED_PARAMS: &[&str] = &["limit", "offset", "sort", "order", "linked"];

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
            "$in" | "$nin" => {
                let arr = operand
                    .as_array()
                    .ok_or_else(|| ApiError::BadRequest(format!("{op} must be an array")))?;
                let (empty_val, kw) = if op == "$in" { ("FALSE", "IN") } else { ("TRUE", "NOT IN") };
                if arr.is_empty() {
                    conditions.push(empty_val.into());
                } else {
                    let phs: Vec<String> = arr
                        .iter()
                        .map(|v| {
                            *idx += 1;
                            binds.push(json_value_to_string(v));
                            format!("${}{cast}", *idx)
                        })
                        .collect();
                    conditions.push(format!("{col} {kw} ({})", phs.join(", ")));
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
    #[serde(default)]
    linked: Option<LinkedOption>,
}

#[derive(Deserialize)]
#[serde(untagged)]
pub enum LinkedOption {
    All(bool),
    Apps(Vec<String>),
}

fn default_query_limit() -> i64 {
    100
}

pub async fn list_records(
    State(rt): State<SharedRuntime>,
    Path((app_id, entity)): Path<(String, String)>,
    identity: Identity,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Vec<JsonValue>>, ApiError> {
    validate_app_id(&app_id)?;
    let pool = pool(&rt).await?;
    require_perm(&pool, &app_id, identity.user_id, &format!("{entity}.read")).await?;
    let tbl = table(&app_id, &entity);
    let types = field_type_map(&pool, &app_id, &entity).await?;

    let (mut idx, mut conditions, mut binds) = (0usize, Vec::new(), Vec::new());

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
    let mut data: Vec<JsonValue> = rows.into_iter().map(|(r,)| r).collect();

    if let Some(lp) = params.get("linked") {
        let linked = match lp.as_str() {
            "true" => LinkedOption::All(true),
            apps => LinkedOption::Apps(apps.split(',').map(String::from).collect()),
        };
        enrich_linked(&pool, identity.user_id, &app_id, &entity, &mut data, &linked).await?;
    }

    Ok(Json(data))
}

async fn enrich_linked(
    pool: &PgPool,
    user_id: Uuid,
    source_app: &str,
    entity: &str,
    rows: &mut [JsonValue],
    linked: &LinkedOption,
) -> Result<(), ApiError> {
    let Some((identity_kind, identity_key)) = entity_identity(pool, source_app, entity)
        .await.map_err(|e| ApiError::Internal(e.to_string()))?
    else { return Ok(()) };

    let targets: Vec<_> = find_entities_by_identity(pool, &identity_kind, Some(source_app))
        .await.map_err(|e| ApiError::Internal(e.to_string()))?
        .into_iter()
        .filter(|(app, _, _)| match linked {
            LinkedOption::All(true) => true,
            LinkedOption::Apps(apps) => apps.contains(app),
            _ => false,
        })
        .collect();

    let key_values: Vec<String> = rows.iter()
        .filter_map(|r| r.get(&identity_key).and_then(|v| v.as_str()).map(String::from))
        .collect();

    if targets.is_empty() || key_values.is_empty() { return Ok(()) }

    for (target_app, target_entity, target_key) in &targets {
        if require_perm(pool, target_app, user_id, &format!("{target_entity}.read")).await.is_err() {
            continue;
        }

        let tbl = table(target_app, target_entity);
        let phs: String = (1..=key_values.len()).map(|i| format!("${i}")).collect::<Vec<_>>().join(",");
        let q = format!("SELECT to_jsonb(t.*) AS row FROM {tbl} t WHERE t.{} IN ({phs})", quote_ident(target_key));
        let mut query = sqlx::query_as::<_, (JsonValue,)>(&q);
        for v in &key_values { query = query.bind(v.as_str()); }
        let Ok(linked_rows) = query.fetch_all(pool).await else { continue };

        let by_key: HashMap<&str, &JsonValue> = linked_rows.iter()
            .filter_map(|(row,)| row.get(target_key).and_then(|v| v.as_str()).map(|k| (k, row)))
            .collect();

        for row in rows.iter_mut() {
            let kv = row.get(&identity_key).and_then(|v| v.as_str()).unwrap_or_default();
            if let Some(&linked_record) = by_key.get(kv) {
                row.as_object_mut().unwrap()
                    .entry("_linked")
                    .or_insert_with(|| serde_json::json!({}))
                    .as_object_mut().unwrap()
                    .insert(target_app.clone(), serde_json::json!({
                        "entity": target_entity,
                        "recordId": linked_record.get("id"),
                        "data": linked_record
                    }));
            }
        }
    }

    Ok(())
}

pub async fn query_records(
    State(rt): State<SharedRuntime>,
    Path((app_id, entity)): Path<(String, String)>,
    identity: Identity,
    Json(body): Json<QueryRequest>,
) -> Result<Json<JsonValue>, ApiError> {
    validate_app_id(&app_id)?;
    let pool = pool(&rt).await?;
    require_perm(&pool, &app_id, identity.user_id, &format!("{entity}.read")).await?;
    let tbl = table(&app_id, &entity);
    let types = field_type_map(&pool, &app_id, &entity).await?;

    let (mut idx, mut conditions, mut binds) = (0usize, Vec::new(), Vec::new());

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
    let mut data: Vec<JsonValue> = rows.into_iter().map(|(r, _)| r).collect();

    if let Some(ref linked) = body.linked {
        enrich_linked(&pool, identity.user_id, &app_id, &entity, &mut data, linked).await?;
    }

    Ok(Json(serde_json::json!({ "data": data, "total": total })))
}

pub async fn create_record(
    State(rt): State<SharedRuntime>,
    Path((app_id, entity)): Path<(String, String)>,
    identity: Identity,
    Json(body): Json<JsonValue>,
) -> Result<(StatusCode, Json<JsonValue>), ApiError> {
    validate_app_id(&app_id)?;
    let pool = pool(&rt).await?;
    require_perm(&pool, &app_id, identity.user_id, &format!("{entity}.create")).await?;
    let obj = require_object(&body)?;
    let tbl = table(&app_id, &entity);
    let types = field_type_map(&pool, &app_id, &entity).await?;

    let mut cols: Vec<String> = Vec::new();
    let mut phs: Vec<String> = Vec::new();
    let mut idx = 1usize;

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
    for (k, v) in obj.iter() {
        query = bind_typed(query, v, types.get(k.as_str()));
    }

    let (row,) = query.fetch_one(&pool).await?;
    Ok((StatusCode::CREATED, Json(row)))
}

fn union_keys(objects: &[&serde_json::Map<String, JsonValue>]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut keys = Vec::new();
    for obj in objects {
        for k in obj.keys() {
            if seen.insert(k) {
                keys.push(k.clone());
            }
        }
    }
    keys
}

pub(crate) async fn bulk_insert(
    pool: &PgPool,
    tbl: &str,
    types: &HashMap<String, String>,
    objects: &[&serde_json::Map<String, JsonValue>],
) -> Result<Vec<JsonValue>, ApiError> {
    let keys = union_keys(objects);
    let total_params = objects.len() * keys.len();
    if total_params > PG_PARAM_LIMIT {
        return Err(ApiError::BadRequest(format!(
            "bulk insert requires {total_params} params, limit is {PG_PARAM_LIMIT}"
        )));
    }

    let cols: Vec<String> = keys.iter().map(|k| quote_ident(k)).collect();
    let ncols = keys.len();
    let mut value_tuples = Vec::with_capacity(objects.len());
    for i in 0..objects.len() {
        let base = i * ncols + 1;
        let phs: Vec<String> = (base..base + ncols).map(|j| format!("${j}")).collect();
        value_tuples.push(format!("({})", phs.join(",")));
    }

    let sql = format!(
        "INSERT INTO {tbl} ({}) VALUES {} RETURNING to_jsonb({tbl}.*) AS row",
        cols.join(","),
        value_tuples.join(",")
    );

    let null_val = JsonValue::Null;
    let mut query = sqlx::query_as::<_, (JsonValue,)>(&sql);
    for obj in objects {
        for key in &keys {
            query = bind_typed(query, obj.get(key.as_str()).unwrap_or(&null_val), types.get(key.as_str()));
        }
    }

    let mut tx = pool.begin().await?;
    let rows: Vec<(JsonValue,)> = query.fetch_all(&mut *tx).await?;
    tx.commit().await?;
    Ok(rows.into_iter().map(|(r,)| r).collect())
}

pub async fn bulk_create_records(
    State(rt): State<SharedRuntime>,
    Path((app_id, entity)): Path<(String, String)>,
    identity: Identity,
    Json(body): Json<JsonValue>,
) -> Result<(StatusCode, Json<Vec<JsonValue>>), ApiError> {
    validate_app_id(&app_id)?;
    let db = pool(&rt).await?;
    require_perm(&db, &app_id, identity.user_id, &format!("{entity}.create")).await?;
    let records = body.as_array()
        .ok_or_else(|| ApiError::BadRequest("body must be a JSON array".into()))?;
    if records.is_empty() {
        return Err(ApiError::BadRequest("array must not be empty".into()));
    }
    if records.len() > MAX_BULK_SIZE {
        return Err(ApiError::BadRequest(format!(
            "batch size {} exceeds max {MAX_BULK_SIZE}", records.len()
        )));
    }
    let objects: Vec<&serde_json::Map<String, JsonValue>> = records.iter()
        .map(|r| require_object(r))
        .collect::<Result<_, _>>()?;

    let tbl = table(&app_id, &entity);
    let types = field_type_map(&db, &app_id, &entity).await?;
    let created = bulk_insert(&db, &tbl, &types, &objects).await?;
    Ok((StatusCode::CREATED, Json(created)))
}

pub async fn get_record(
    State(rt): State<SharedRuntime>,
    Path((app_id, entity, id)): Path<(String, String, String)>,
    identity: Identity,
) -> Result<Json<JsonValue>, ApiError> {
    validate_app_id(&app_id)?;
    let (uuid, pool) = (parse_uuid(&id)?, pool(&rt).await?);
    require_perm(&pool, &app_id, identity.user_id, &format!("{entity}.read")).await?;
    let tbl = table(&app_id, &entity);
    let q = format!("SELECT to_jsonb(t.*) AS row FROM {tbl} t WHERE t.id = $1");
    let query = sqlx::query_as::<_, (JsonValue,)>(&q).bind(uuid);
    query
        .fetch_optional(&pool)
        .await?
        .map(|(r,)| Json(r))
        .ok_or_else(|| ApiError::NotFound(format!("record '{id}' not found")))
}

pub async fn update_record(
    State(rt): State<SharedRuntime>,
    Path((app_id, entity, id)): Path<(String, String, String)>,
    identity: Identity,
    Json(body): Json<JsonValue>,
) -> Result<Json<JsonValue>, ApiError> {
    validate_app_id(&app_id)?;
    let (uuid, pool) = (parse_uuid(&id)?, pool(&rt).await?);
    require_perm(&pool, &app_id, identity.user_id, &format!("{entity}.update")).await?;
    let obj = require_object(&body)?;
    let tbl = table(&app_id, &entity);
    let types = field_type_map(&pool, &app_id, &entity).await?;
    let entries: Vec<(&str, &JsonValue)> = obj.iter().map(|(k, v)| (k.as_str(), v)).collect();
    let sets: Vec<String> =
        entries.iter().enumerate().map(|(i, (k, _))| format!("{} = ${}", quote_ident(k), i + 1)).collect();
    let id_param = entries.len() + 1;
    let q = format!(
        "UPDATE {tbl} SET {}, \"updated_at\" = now() WHERE id = ${id_param} RETURNING to_jsonb({tbl}.*) AS row",
        sets.join(", ")
    );
    let mut query = sqlx::query_as::<_, (JsonValue,)>(&q);
    for (k, v) in &entries {
        query = bind_typed(query, v, types.get(*k));
    }
    query = query.bind(uuid);
    query
        .fetch_optional(&pool)
        .await?
        .map(|(r,)| Json(r))
        .ok_or_else(|| ApiError::NotFound(format!("record '{id}' not found")))
}

pub async fn delete_record(
    State(rt): State<SharedRuntime>,
    Path((app_id, entity, id)): Path<(String, String, String)>,
    identity: Identity,
) -> Result<Json<JsonValue>, ApiError> {
    validate_app_id(&app_id)?;
    let (uuid, pool) = (parse_uuid(&id)?, pool(&rt).await?);
    require_perm(&pool, &app_id, identity.user_id, &format!("{entity}.delete")).await?;
    let tbl = table(&app_id, &entity);
    let q = format!("DELETE FROM {tbl} WHERE id = $1");
    let r = sqlx::query(&q).bind(uuid).execute(&pool).await?;
    if r.rows_affected() == 0 {
        return Err(ApiError::NotFound(format!("record '{id}' not found")));
    }
    Ok(Json(serde_json::json!({ "message": format!("record '{id}' deleted") })))
}

pub async fn federated_query(
    State(rt): State<SharedRuntime>,
    Path(identity_kind): Path<String>,
    identity: Identity,
    Json(body): Json<QueryRequest>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = pool(&rt).await?;
    let targets = find_entities_by_identity(&pool, &identity_kind, None)
        .await.map_err(|e| ApiError::Internal(e.to_string()))?;

    let mut unions: Vec<String> = Vec::new();
    let mut all_binds: Vec<String> = Vec::new();
    let mut bind_offset = 0usize;

    for (app_id, entity_name, _key) in &targets {
        if require_perm(&pool, app_id, identity.user_id, &format!("{entity_name}.read")).await.is_err() {
            continue;
        }

        let tbl = table(app_id, entity_name);
        let types = field_type_map(&pool, app_id, entity_name).await?;
        let (mut idx, mut conditions, mut binds) = (bind_offset, Vec::new(), Vec::new());

        if let Some(ref w) = body.where_clause {
            let sql = build_where_clause(w, &types, &mut binds, &mut idx)?;
            if sql != "TRUE" { conditions.push(sql); }
        }

        let wh = join_where(&conditions);
        // _source built via bind params to avoid SQL injection from app_id/entity_name
        idx += 1;
        let app_bind = idx;
        binds.push(app_id.clone());
        idx += 1;
        let ent_bind = idx;
        binds.push(entity_name.clone());

        unions.push(format!(
            "SELECT to_jsonb(t.*) || jsonb_build_object('_source', jsonb_build_object('app', ${app_bind}::text, 'entity', ${ent_bind}::text)) AS row FROM {tbl} t{wh}"
        ));

        bind_offset = idx;
        all_binds.extend(binds);
    }

    if unions.is_empty() {
        return Ok(Json(serde_json::json!({ "data": [], "total": 0 })));
    }

    let limit = body.limit.min(1000).max(1);
    let offset = body.offset.max(0);
    let sort_field = body.order_by.as_deref().unwrap_or("created_at");
    if !sort_field.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_') {
        return Err(ApiError::BadRequest(format!("invalid sort field: '{sort_field}'")));
    }
    let order = validate_order(body.order.as_ref());

    let q = format!(
        "SELECT row, COUNT(*) OVER() AS total FROM ({}) sub \
         ORDER BY row->>'{sort_field}' {order} LIMIT {limit} OFFSET {offset}",
        unions.join(" UNION ALL ")
    );

    let mut query = sqlx::query_as::<_, (JsonValue, i64)>(&q);
    for s in &all_binds { query = query.bind(s.as_str()); }
    let rows: Vec<(JsonValue, i64)> = query.fetch_all(&pool).await?;

    let total = rows.first().map(|(_, t)| *t).unwrap_or(0);
    let data: Vec<JsonValue> = rows.into_iter().map(|(r, _)| r).collect();
    Ok(Json(serde_json::json!({ "data": data, "total": total })))
}

pub(crate) type QA<'q> = sqlx::query::QueryAs<'q, sqlx::Postgres, (JsonValue,), sqlx::postgres::PgArguments>;

/// Bind using the manifest type (not the JSON payload) so the parameter OID
/// is stable across sqlx's prepared-statement cache.
pub(crate) fn bind_typed<'q>(q: QA<'q>, val: &'q JsonValue, manifest_type: Option<&String>) -> QA<'q> {
    let pg = manifest_type.map(|t| map_field_type(t));

    if val.is_null() {
        return match pg {
            Some("DOUBLE PRECISION") => q.bind(None::<f64>),
            Some("BOOLEAN")         => q.bind(None::<bool>),
            Some("UUID")            => q.bind(None::<Uuid>),
            Some("DATE")            => q.bind(None::<NaiveDate>),
            Some("TIMESTAMPTZ")     => q.bind(None::<DateTime<Utc>>),
            Some("JSONB")           => q.bind(None::<JsonValue>),
            Some("TEXT[]")          => q.bind(None::<Vec<String>>),
            Some("DOUBLE PRECISION[]") => q.bind(None::<Vec<f64>>),
            _ => q.bind(None::<String>),
        };
    }

    match pg {
        Some("DOUBLE PRECISION") => q.bind(coerce_f64(val)),
        Some("BOOLEAN") => q.bind(coerce_bool(val)),
        Some("UUID") => q.bind(coerce_str(val).and_then(|s| s.parse::<Uuid>().ok())),
        Some("DATE") => q.bind(coerce_str(val).and_then(|s| s.parse::<NaiveDate>().ok())),
        Some("TIMESTAMPTZ") => q.bind(coerce_str(val).and_then(|s| s.parse::<DateTime<Utc>>().ok())),
        Some("JSONB") => q.bind(val),
        Some("TEXT[]") => q.bind(coerce_str_vec(val)),
        Some("DOUBLE PRECISION[]") => q.bind(coerce_f64_vec(val)),
        _ => match val {
            JsonValue::String(s) => q.bind(s.as_str()),
            _ => q.bind(json_value_to_string(val)),
        },
    }
}

fn coerce_f64(val: &JsonValue) -> f64 {
    match val {
        JsonValue::Number(n) => n.as_f64().unwrap_or(0.0),
        JsonValue::String(s) => s.parse().unwrap_or(0.0),
        JsonValue::Bool(b) => if *b { 1.0 } else { 0.0 },
        _ => 0.0,
    }
}

fn coerce_bool(val: &JsonValue) -> bool {
    match val {
        JsonValue::Bool(b) => *b,
        JsonValue::Number(n) => n.as_i64().is_some_and(|i| i != 0),
        JsonValue::String(s) => matches!(s.as_str(), "true" | "1"),
        _ => false,
    }
}

fn coerce_str(val: &JsonValue) -> Option<&str> {
    val.as_str()
}

fn coerce_str_vec<'a>(val: &'a JsonValue) -> Vec<&'a str> {
    match val {
        JsonValue::Array(arr) => arr.iter().filter_map(|v| v.as_str()).collect(),
        _ => Vec::new(),
    }
}

fn coerce_f64_vec(val: &JsonValue) -> Vec<f64> {
    match val {
        JsonValue::Array(arr) => arr.iter().filter_map(|v| v.as_f64()).collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn where_nin_operator() {
        let types = types_fixture();
        let mut binds = Vec::new();
        let mut idx = 0;
        let sql = build_where_clause(
            &json!({"status": {"$nin": ["deleted", "archived"]}}),
            &types,
            &mut binds,
            &mut idx,
        )
        .unwrap();
        assert_eq!(sql, "\"status\" NOT IN ($1, $2)");
        assert_eq!(binds, vec!["deleted", "archived"]);
    }

    #[test]
    fn where_nin_empty_is_true() {
        let types = types_fixture();
        let mut binds = Vec::new();
        let mut idx = 0;
        let sql =
            build_where_clause(&json!({"status": {"$nin": []}}), &types, &mut binds, &mut idx)
                .unwrap();
        assert_eq!(sql, "TRUE");
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
