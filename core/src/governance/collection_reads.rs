//! Collection reads on an already-governed connection. This module never
//! acquires a pool or installs authority; remote callers must authorize the full
//! requested field scope before entering, then project the grant on return.

use serde_json::Value as JsonValue;
use sqlx::PgConnection;

use super::cross_app;
use crate::data_types::{FieldTypes, bind_typed, row_json};
use crate::manifest::quote_ident;
use crate::routes::crud::{build_where_clause, join_where, table, validate_sort_field};

pub(crate) const PAGE_OPTION_KEYS: [&str; 5] = ["where", "orderBy", "order", "limit", "offset"];

#[derive(Clone, Copy)]
pub(crate) enum EqualityMode {
    All,
    NewestFirst,
    One,
}

fn validate_field(types: &FieldTypes, name: &str) -> Result<(), String> {
    match types.get(name) {
        Some(field) if !field.sensitive => Ok(()),
        Some(_) => Err(format!("field '{name}' is sensitive and cannot be queried")),
        None => Err(format!("unknown field '{name}'")),
    }
}

pub(crate) async fn equality(
    conn: &mut PgConnection,
    types: &FieldTypes,
    app_id: &str,
    entity: &str,
    data: JsonValue,
    mode: EqualityMode,
) -> Result<JsonValue, String> {
    let object = data
        .as_object()
        .ok_or("data must be a JSON object (where clause)")?;
    for name in object.keys() {
        validate_field(types, name)?;
    }
    let tbl = table(app_id, entity);
    let conditions: Vec<String> = object
        .keys()
        .enumerate()
        .map(|(index, name)| format!("{} = ${}", quote_ident(name), index + 1))
        .collect();
    let where_clause = join_where(&conditions);
    let suffix = match mode {
        EqualityMode::All => "",
        EqualityMode::NewestFirst => " ORDER BY \"created_at\" DESC",
        EqualityMode::One => " LIMIT 1",
    };
    let sql = format!(
        "SELECT {} AS row FROM {tbl}{where_clause}{suffix}",
        row_json(&tbl, types)
    );
    let mut query = sqlx::query_as::<_, (JsonValue,)>(&sql);
    for (name, value) in object {
        query = bind_typed(query, value, types.get(name).map(|field| &field.field_type))?;
    }
    if matches!(mode, EqualityMode::One) {
        return Ok(query
            .fetch_optional(&mut *conn)
            .await
            .map_err(|e| e.to_string())?
            .map_or(JsonValue::Null, |(row,)| row));
    }
    let rows = query
        .fetch_all(&mut *conn)
        .await
        .map_err(|e| e.to_string())?;
    Ok(JsonValue::Array(
        rows.into_iter().map(|(row,)| row).collect(),
    ))
}

/// Legacy remote find/list accept either an equality map or page options.
/// New findPage always accepts only the page-options envelope.
pub(crate) async fn page(
    conn: &mut PgConnection,
    types: &FieldTypes,
    app_id: &str,
    entity: &str,
    data: JsonValue,
    legacy: bool,
) -> Result<JsonValue, String> {
    let object = data
        .as_object()
        .ok_or("data must be a JSON object (where clause)")?;
    let mut binds = Vec::new();
    let mut index = 0usize;
    let uses_query_options =
        !legacy || PAGE_OPTION_KEYS.iter().any(|key| object.contains_key(*key));
    if uses_query_options
        && object
            .keys()
            .any(|key| !PAGE_OPTION_KEYS.contains(&key.as_str()))
    {
        return Err(
            "query options cannot be mixed with legacy equality fields; put filters under 'where'"
                .into(),
        );
    }
    let empty_where = JsonValue::Object(serde_json::Map::new());
    let where_clause = if uses_query_options {
        object.get("where").unwrap_or(&empty_where)
    } else {
        &data
    };
    for name in cross_app::query_field_names(
        Some(where_clause),
        object.get("orderBy").and_then(JsonValue::as_str),
    ) {
        validate_field(types, &name)?;
    }
    let where_sql = build_where_clause(where_clause, types, &mut binds, &mut index)
        .map_err(|error| format!("{error:?}"))?;
    let conditions = if where_sql == "TRUE" {
        Vec::new()
    } else {
        vec![where_sql]
    };
    let (order_by, direction, limit, offset) = cross_app::parse_query_options(object)?;
    let sort = validate_sort_field(order_by.as_ref(), types);
    let tbl = table(app_id, entity);
    let row = row_json("t", types);
    let where_sql = join_where(&conditions);
    // Both subqueries share one PostgreSQL statement snapshot. Only the page
    // is serialized to JSON; counting does not materialize every provider row.
    let sql = format!(
        "WITH page AS (
            SELECT {row} AS row, {sort} AS sort_value FROM {tbl} t{where_sql}
            ORDER BY {sort} {direction} LIMIT {limit} OFFSET {offset}
         )
         SELECT jsonb_build_object(
            'data', COALESCE((SELECT jsonb_agg(row ORDER BY sort_value {direction}) FROM page), '[]'::jsonb),
            'total', (SELECT count(*) FROM {tbl} t{where_sql})
         )",
    );
    let mut query = sqlx::query_as::<_, (JsonValue,)>(&sql);
    for bind in &binds {
        query = query.bind(bind.as_str());
    }
    let (page,) = query
        .fetch_one(&mut *conn)
        .await
        .map_err(|error| error.to_string())?;
    Ok(page)
}

/// A publication keeps its row predicate and projection together. Neither is
/// taken from the caller's query, and the count uses the same scoped statement.
pub(crate) async fn published(
    conn: &mut PgConnection,
    types: &FieldTypes,
    app_id: &str,
    entity: &str,
    fields: &[String],
    predicate: &JsonValue,
    query_filter: &JsonValue,
    options: &serde_json::Map<String, JsonValue>,
    mode: PublicationRead,
) -> Result<JsonValue, String> {
    let mut binds = Vec::new();
    let mut index = 0;
    let fixed = build_where_clause(predicate, types, &mut binds, &mut index)
        .map_err(|e| format!("{e:?}"))?;
    let requested = build_where_clause(query_filter, types, &mut binds, &mut index)
        .map_err(|e| format!("{e:?}"))?;
    let scope = format!("({fixed}) AND ({requested})");
    let tbl = table(app_id, entity);
    let projection = publication_projection(types, fields);
    let (order_by, direction, limit, offset) = cross_app::parse_query_options(options)?;
    let sort = order_by.as_ref().unwrap_or(&fields[0]);
    if !fields.contains(sort) {
        return Err("publication does not allow this sort field".into());
    }
    let sort = quote_ident(sort);
    let sql = match mode {
        PublicationRead::Page => format!(
            "WITH page AS (
               SELECT {projection} AS row, {sort} AS sort_value FROM {tbl} t
               WHERE {scope} ORDER BY {sort} {direction} LIMIT {limit} OFFSET {offset}
             ) SELECT jsonb_build_object(
               'data', COALESCE((SELECT jsonb_agg(row ORDER BY sort_value {direction}) FROM page), '[]'::jsonb),
               'total', (SELECT count(*) FROM {tbl} t WHERE {scope}))"
        ),
        PublicationRead::One => format!("SELECT {projection} FROM {tbl} t WHERE {scope} LIMIT 1"),
        PublicationRead::All => format!(
            "SELECT {projection} FROM {tbl} t WHERE {scope} ORDER BY {sort} {direction} LIMIT 10001"
        ),
    };
    let mut query = sqlx::query_as::<_, (JsonValue,)>(&sql);
    for bind in &binds {
        query = query.bind(bind);
    }
    let result = match mode {
        PublicationRead::One => query
            .fetch_optional(&mut *conn)
            .await
            .map_err(|e| e.to_string())?
            .map_or(JsonValue::Null, |(row,)| row),
        PublicationRead::Page => {
            query
                .fetch_one(&mut *conn)
                .await
                .map_err(|e| e.to_string())?
                .0
        }
        PublicationRead::All => {
            let rows = query
                .fetch_all(&mut *conn)
                .await
                .map_err(|e| e.to_string())?;
            if rows.len() > 10_000 {
                return Err("public read exceeds 10000 rows; use findPage".into());
            }
            JsonValue::Array(rows.into_iter().map(|(row,)| row).collect())
        }
    };
    if result.to_string().len() > 4 * 1024 * 1024 {
        return Err("public read exceeds the 4 MiB response limit; use a smaller page".into());
    }
    Ok(result)
}

fn publication_projection(types: &FieldTypes, fields: &[String]) -> String {
    // Chunk the projection to stay below PostgreSQL's function argument limit.
    fields
        .chunks(40)
        .map(|chunk| {
            let entries = chunk
                .iter()
                .map(|name| {
                    let cast = if types
                        .get(name)
                        .is_some_and(|field| field.field_type.is_decimal())
                    {
                        "::text"
                    } else {
                        ""
                    };
                    format!(
                        "{}, t.{}{cast}",
                        crate::manifest::quote_literal(name),
                        quote_ident(name),
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("jsonb_build_object({entries})")
        })
        .collect::<Vec<_>>()
        .join(" || ")
}

#[derive(Clone, Copy)]
pub(crate) enum PublicationRead {
    All,
    One,
    Page,
}
