use std::collections::HashMap;

use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sqlx::postgres::{PgColumn, PgRow, PgTypeInfo};
use sqlx::{Column, Row as _, TypeInfo, ValueRef};

use super::{pool, SharedRuntime};
use crate::api_error::ApiError;

#[derive(Serialize)]
pub struct SchemaInfo {
    pub schema_name: String,
    pub table_count: i64,
}

pub async fn list_schemas(
    State(rt): State<SharedRuntime>,
) -> Result<Json<Vec<SchemaInfo>>, ApiError> {
    let pool = pool(&rt).await?;
    let rows = sqlx::query_as::<_, (String, i64)>(
        r#"
        SELECT s.schema_name, COUNT(t.table_name)::bigint AS table_count
        FROM information_schema.schemata s
        LEFT JOIN information_schema.tables t
            ON t.table_schema = s.schema_name
            AND t.table_type = 'BASE TABLE'
        WHERE s.schema_name NOT IN ('pg_catalog', 'information_schema', 'pg_toast')
          AND s.schema_name NOT LIKE 'pg_%'
        GROUP BY s.schema_name
        ORDER BY
            CASE WHEN s.schema_name = 'rootcx_system' THEN 0
                 WHEN s.schema_name = 'public' THEN 1
                 ELSE 2 END,
            s.schema_name
        "#,
    )
    .fetch_all(&pool)
    .await?;

    Ok(Json(
        rows.into_iter()
            .map(|(schema_name, table_count)| SchemaInfo { schema_name, table_count })
            .collect(),
    ))
}

#[derive(Serialize)]
pub struct ColumnInfo {
    pub column_name: String,
    pub data_type: String,
    pub is_nullable: bool,
    pub column_default: Option<String>,
    pub ordinal_position: i32,
}

#[derive(Serialize)]
pub struct TableInfo {
    pub table_name: String,
    pub columns: Vec<ColumnInfo>,
    pub row_estimate: i64,
}

pub async fn list_tables(
    State(rt): State<SharedRuntime>,
    Path(schema): Path<String>,
) -> Result<Json<Vec<TableInfo>>, ApiError> {
    let pool = pool(&rt).await?;

    let tables: Vec<(String,)> = sqlx::query_as(
        r#"
        SELECT table_name
        FROM information_schema.tables
        WHERE table_schema = $1 AND table_type = 'BASE TABLE'
        ORDER BY table_name
        "#,
    )
    .bind(&schema)
    .fetch_all(&pool)
    .await?;

    let columns: Vec<(String, String, String, String, Option<String>, i32)> = sqlx::query_as(
        r#"
        SELECT c.table_name, c.column_name, c.data_type, c.is_nullable,
               c.column_default, c.ordinal_position::int
        FROM information_schema.columns c
        WHERE c.table_schema = $1
        ORDER BY c.table_name, c.ordinal_position
        "#,
    )
    .bind(&schema)
    .fetch_all(&pool)
    .await?;

    let estimates: Vec<(String, i64)> = sqlx::query_as(
        r#"
        SELECT relname::text, reltuples::bigint
        FROM pg_class c
        JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE n.nspname = $1 AND c.relkind = 'r'
        "#,
    )
    .bind(&schema)
    .fetch_all(&pool)
    .await?;

    let estimate_map: HashMap<String, i64> = estimates.into_iter().collect();
    let mut table_columns: HashMap<String, Vec<ColumnInfo>> = HashMap::new();

    for (table_name, column_name, data_type, is_nullable, column_default, ordinal_position) in
        columns
    {
        table_columns
            .entry(table_name)
            .or_default()
            .push(ColumnInfo {
                column_name,
                data_type,
                is_nullable: is_nullable == "YES",
                column_default,
                ordinal_position,
            });
    }

    Ok(Json(
        tables
            .into_iter()
            .map(|(table_name,)| {
                let columns = table_columns.remove(&table_name).unwrap_or_default();
                let row_estimate = estimate_map.get(&table_name).copied().unwrap_or(0).max(0);
                TableInfo { table_name, columns, row_estimate }
            })
            .collect(),
    ))
}

const MAX_ROWS: usize = 500;

#[derive(Deserialize)]
pub struct QueryRequest {
    pub sql: String,
    #[serde(default)]
    pub schema: Option<String>,
}

#[derive(Serialize)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<JsonValue>>,
    pub row_count: usize,
}

pub async fn execute_query(
    State(rt): State<SharedRuntime>,
    Json(body): Json<QueryRequest>,
) -> Result<Json<QueryResult>, ApiError> {
    let pool = pool(&rt).await?;

    let sql = body.sql.trim();
    if sql.is_empty() {
        return Err(ApiError::BadRequest("empty query".into()));
    }

    // Dedicated connection so SET search_path doesn't leak to the pool
    let mut conn = pool
        .acquire()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    if let Some(ref schema) = body.schema {
        sqlx::query(&format!(
            "SET search_path TO {}, public",
            crate::manifest::quote_ident(schema)
        ))
        .execute(&mut *conn)
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    }

    let rows = sqlx::query(sql)
        .fetch_all(&mut *conn)
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;

    if rows.is_empty() {
        return Ok(Json(QueryResult { columns: vec![], rows: vec![], row_count: 0 }));
    }

    let columns: Vec<String> =
        rows[0].columns().iter().map(|c: &PgColumn| c.name().to_string()).collect();

    let capped = rows.len().min(MAX_ROWS);
    let json_rows: Vec<Vec<JsonValue>> = rows[..capped]
        .iter()
        .map(|row| row.columns().iter().enumerate().map(|(i, col)| pg_val(row, i, col.type_info())).collect())
        .collect();

    Ok(Json(QueryResult { row_count: json_rows.len(), columns, rows: json_rows }))
}

fn try_json<'r, T: sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>>(
    row: &'r PgRow,
    idx: usize,
    f: impl FnOnce(T) -> JsonValue,
) -> JsonValue {
    row.try_get::<T, _>(idx).map(f).unwrap_or(JsonValue::Null)
}

fn pg_val(row: &PgRow, idx: usize, ti: &PgTypeInfo) -> JsonValue {
    use sqlx::types::chrono;

    if row.try_get_raw(idx).map(|v| v.is_null()).unwrap_or(true) {
        return JsonValue::Null;
    }

    match ti.name() {
        "BOOL" => try_json(row, idx, JsonValue::Bool),
        "INT2" => try_json::<i16>(row, idx, |v| JsonValue::Number(v.into())),
        "INT4" => try_json::<i32>(row, idx, |v| JsonValue::Number(v.into())),
        "INT8" => try_json::<i64>(row, idx, |v| JsonValue::Number(v.into())),
        "FLOAT4" => row
            .try_get::<f32, _>(idx)
            .ok()
            .and_then(|v| serde_json::Number::from_f64(v as f64))
            .map(JsonValue::Number)
            .unwrap_or(JsonValue::Null),
        "FLOAT8" => row
            .try_get::<f64, _>(idx)
            .ok()
            .and_then(|v| serde_json::Number::from_f64(v))
            .map(JsonValue::Number)
            .unwrap_or(JsonValue::Null),
        "JSONB" | "JSON" => row.try_get::<JsonValue, _>(idx).unwrap_or(JsonValue::Null),
        "UUID" => try_json::<sqlx::types::Uuid>(row, idx, |v| JsonValue::String(v.to_string())),
        "TIMESTAMPTZ" => try_json::<chrono::DateTime<chrono::Utc>>(row, idx, |v| JsonValue::String(v.to_rfc3339())),
        "TIMESTAMP" => try_json::<chrono::NaiveDateTime>(row, idx, |v| JsonValue::String(v.to_string())),
        "DATE" => try_json::<chrono::NaiveDate>(row, idx, |v| JsonValue::String(v.to_string())),
        "TEXT[]" | "_TEXT" => try_json::<Vec<String>>(row, idx, |v| {
            JsonValue::Array(v.into_iter().map(JsonValue::String).collect())
        }),
        _ => row
            .try_get::<String, _>(idx)
            .map(JsonValue::String)
            .unwrap_or_else(|_| JsonValue::String(format!("<{}>", ti.name()))),
    }
}
