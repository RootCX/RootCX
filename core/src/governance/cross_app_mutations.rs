use serde_json::{Map, Value as JsonValue, json};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::data_types::{FieldTypes, bind_typed, row_json};
use crate::manifest::quote_ident;
use crate::routes::crud::{MAX_BULK_SIZE, table};

type Object = Map<String, JsonValue>;

enum Mutation<'a> {
    Create(&'a Object),
    BulkCreate(Vec<&'a Object>),
    Update(Uuid, &'a Object),
    Delete(Uuid),
}

fn writable_object<'a>(value: &'a JsonValue, types: &FieldTypes) -> Result<&'a Object, String> {
    let object = value.as_object().ok_or("data must be an object")?;
    if object.is_empty() {
        return Err("data must contain at least one writable field".into());
    }
    for name in object.keys() {
        if matches!(name.as_str(), "id" | "created_at" | "updated_at") {
            return Err(format!("field '{name}' is system-managed"));
        }
        match types.get(name) {
            None => return Err(format!("unknown field '{name}'")),
            Some(field) if field.sensitive => {
                return Err(format!("field '{name}' is sensitive and cannot be written"));
            }
            _ => {}
        }
    }
    Ok(object)
}

fn validate<'a>(
    types: &FieldTypes,
    op: &str,
    data: &'a JsonValue,
    idempotency_key: Option<&str>,
) -> Result<Mutation<'a>, String> {
    match op {
        "create" => Ok(Mutation::Create(writable_object(data, types)?)),
        "bulk_create" => {
            if idempotency_key.is_some() {
                return Err(
                    "bulk_create is not allowed inside a workflow; use per-item create".into(),
                );
            }
            let rows = data
                .as_array()
                .ok_or("bulk_create data must be an array of objects")?;
            if rows.is_empty() || rows.len() > MAX_BULK_SIZE {
                return Err(format!("bulk_create requires 1..={MAX_BULK_SIZE} objects"));
            }
            let objects = rows
                .iter()
                .map(|row| writable_object(row, types))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Mutation::BulkCreate(objects))
        }
        "update" | "delete" => {
            let object = data.as_object().ok_or("mutation data must be an object")?;
            for name in object.keys() {
                if name != "id" && !(op == "update" && name == "data") {
                    return Err(format!("unexpected mutation argument '{name}'"));
                }
            }
            let id = object
                .get("id")
                .and_then(JsonValue::as_str)
                .ok_or("id must be an explicit UUID string")?
                .parse::<Uuid>()
                .map_err(|_| "id must be a valid UUID".to_string())?;
            if op == "delete" {
                Ok(Mutation::Delete(id))
            } else {
                let patch = object.get("data").ok_or("update requires a data object")?;
                Ok(Mutation::Update(id, writable_object(patch, types)?))
            }
        }
        _ => Err(format!("unsupported mutation operation '{op}'")),
    }
}

async fn insert(
    conn: &mut PgConnection,
    types: &FieldTypes,
    table: &str,
    object: &Object,
    idempotency_key: Option<&str>,
) -> Result<JsonValue, String> {
    let mut columns: Vec<_> = object.keys().map(|name| quote_ident(name)).collect();
    let mut placeholders: Vec<_> = (1..=object.len())
        .map(|index| format!("${index}"))
        .collect();
    let id = idempotency_key.map(|key| Uuid::new_v5(&Uuid::NAMESPACE_OID, key.as_bytes()));
    let conflict = if id.is_some() {
        columns.push(quote_ident("id"));
        placeholders.push(format!("${}", placeholders.len() + 1));
        "ON CONFLICT (\"id\") DO NOTHING"
    } else {
        ""
    };
    let projection = row_json("t", types);
    let sql = format!(
        "INSERT INTO {table} AS t ({}) VALUES ({}) {conflict} RETURNING {projection}",
        columns.join(", "),
        placeholders.join(", "),
    );
    let mut query = sqlx::query_as::<_, (JsonValue,)>(&sql);
    for (name, value) in object {
        query = bind_typed(query, value, types.get(name).map(|field| &field.field_type))?;
    }
    if let Some(id) = id {
        query = query.bind(id);
    }
    if let Some((row,)) = query
        .fetch_optional(&mut *conn)
        .await
        .map_err(|e| e.to_string())?
    {
        return Ok(row);
    }
    let id = id.ok_or("create returned no row")?;
    // A separate statement sees the committed conflicting row after an insert
    // race. The caller's RLS context still applies; no UPDATE permission is used.
    let sql = format!("SELECT {projection} FROM {table} AS t WHERE t.\"id\" = $1");
    let row = sqlx::query_as::<_, (JsonValue,)>(&sql)
        .bind(id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("record '{id}' not found"))?;
    Ok(row.0)
}

/// Runs only inside the caller's governed transaction and established RLS context.
/// The caller must roll back on error, including an error partway through a batch.
pub(crate) async fn execute(
    conn: &mut PgConnection,
    types: &FieldTypes,
    app: &str,
    entity: &str,
    op: &str,
    data: JsonValue,
    idempotency_key: Option<&str>,
) -> Result<JsonValue, String> {
    let mutation = validate(types, op, &data, idempotency_key)?;
    let table = table(app, entity);
    match mutation {
        Mutation::Create(object) => insert(conn, types, &table, object, idempotency_key).await,
        Mutation::BulkCreate(objects) => {
            let mut rows = Vec::with_capacity(objects.len());
            for object in objects {
                rows.push(insert(conn, types, &table, object, None).await?);
            }
            Ok(JsonValue::Array(rows))
        }
        Mutation::Update(id, object) => {
            let mut assignments: Vec<_> = object
                .keys()
                .enumerate()
                .map(|(index, name)| format!("{} = ${}", quote_ident(name), index + 1))
                .collect();
            assignments.push("\"updated_at\" = now()".into());
            let sql = format!(
                "UPDATE {table} AS t SET {} WHERE t.\"id\" = ${} RETURNING {}",
                assignments.join(", "),
                object.len() + 1,
                row_json("t", types),
            );
            let mut query = sqlx::query_as::<_, (JsonValue,)>(&sql);
            for (name, value) in object {
                query = bind_typed(query, value, types.get(name).map(|field| &field.field_type))?;
            }
            let row = query
                .bind(id)
                .fetch_optional(&mut *conn)
                .await
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("record '{id}' not found"))?;
            Ok(row.0)
        }
        Mutation::Delete(id) => {
            let sql = format!("DELETE FROM {table} WHERE \"id\" = $1");
            let result = sqlx::query(&sql)
                .bind(id)
                .execute(&mut *conn)
                .await
                .map_err(|e| e.to_string())?;
            if result.rows_affected() == 0 && idempotency_key.is_none() {
                return Err(format!("record '{id}' not found"));
            }
            Ok(json!({"id": id, "deleted": true}))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_types::{Field, FieldType};

    fn types() -> FieldTypes {
        [
            ("name", false),
            ("secret", true),
            ("id", false),
            ("created_at", false),
            ("updated_at", false),
        ]
        .into_iter()
        .map(|(name, sensitive)| {
            (
                name.into(),
                Field {
                    field_type: FieldType::Text,
                    sensitive,
                },
            )
        })
        .collect()
    }

    #[test]
    fn caller_writes_cannot_bypass_field_boundaries() {
        let types = types();
        for (field, reason) in [
            ("id", "system-managed"),
            ("created_at", "system-managed"),
            ("updated_at", "system-managed"),
            ("unknown", "unknown"),
            ("secret", "sensitive"),
        ] {
            let value = json!({field: "forbidden", "name": "allowed"});
            for op in ["create", "update", "bulk_create"] {
                let data = match op {
                    "update" => json!({"id": Uuid::nil(), "data": value}),
                    "bulk_create" => json!([{"name": "valid first row"}, value]),
                    _ => value.clone(),
                };
                let error = validate(&types, op, &data, None)
                    .err()
                    .expect("must reject write");
                assert!(error.contains(reason), "{op} {field}: {error}");
            }
        }
    }

    #[test]
    fn update_and_delete_require_explicit_uuid_selectors() {
        let types = types();
        for op in ["update", "delete"] {
            for selector in [
                JsonValue::Null,
                json!(1),
                json!(""),
                json!("bad"),
                json!({"name": "a"}),
            ] {
                let mut data = json!({"id": selector});
                if op == "update" {
                    data["data"] = json!({"name": "a"});
                }
                assert!(validate(&types, op, &data, None).is_err(), "{op}: {data}");
            }
            let mut data = json!({"id": Uuid::nil()});
            if op == "update" {
                data["data"] = json!({"name": "a"});
            }
            assert!(validate(&types, op, &data, None).is_ok(), "{op}: {data}");
            data["where"] = json!({"name": "a"});
            assert!(validate(&types, op, &data, None).is_err(), "{op}: {data}");
        }
    }

    #[test]
    fn bulk_rejects_invalid_batches_and_workflow_retries() {
        let types = types();
        for count in [0, 1, MAX_BULK_SIZE, MAX_BULK_SIZE + 1] {
            let data = JsonValue::Array(vec![json!({"name": "a"}); count]);
            assert_eq!(
                validate(&types, "bulk_create", &data, None).is_ok(),
                (1..=MAX_BULK_SIZE).contains(&count),
                "batch size {count}"
            );
        }
        for data in [json!({}), json!([{"name": "a"}, null]), json!([{}])] {
            assert!(
                validate(&types, "bulk_create", &data, None).is_err(),
                "{data}"
            );
        }
        assert!(
            validate(
                &types,
                "bulk_create",
                &json!([{"name": "a"}]),
                Some("retry")
            )
            .is_err()
        );
    }

    #[test]
    fn empty_or_malformed_writes_are_rejected() {
        let types = types();
        for data in [json!({}), JsonValue::Null, json!([]), json!("name")] {
            assert!(validate(&types, "create", &data, None).is_err(), "{data}");
            let update = json!({"id": Uuid::nil(), "data": data});
            assert!(
                validate(&types, "update", &update, None).is_err(),
                "{update}"
            );
        }
        assert!(validate(&types, "update", &json!({"id": Uuid::nil()}), None).is_err());
        assert!(validate(&types, "delete", &json!({}), None).is_err());
        assert!(validate(&types, "upsert", &json!({"name": "a"}), None).is_err());
        assert!(validate(&types, "create", &json!({"name": "a"}), Some("retry")).is_ok());
    }
}
