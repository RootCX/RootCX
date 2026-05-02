use async_trait::async_trait;
use serde_json::{json, Value as JsonValue};
use rootcx_types::ToolDescriptor;

use super::{Tool, ToolContext, str_arg, check_permission};
use crate::manifest::{field_type_map, quote_ident};
use crate::routes::crud::{bind_typed, bulk_insert, filter_writable_fields, table, MAX_BULK_SIZE};

pub struct MutateDataTool;

#[async_trait]
impl Tool for MutateDataTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "mutate_data".into(),
            description: "Create, update, or delete records. Use bulk_create to insert many records at once. Array fields (type [text], [number]) must be JSON arrays, not strings.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "entity": { "type": "string", "description": "The collection/entity name" },
                    "app": { "type": "string", "description": "Target app ID for cross-app writes" },
                    "action": { "type": "string", "enum": ["create", "update", "delete", "bulk_create"], "description": "The mutation action" },
                    "data": { "description": "A record object (create/update) or array of record objects (bulk_create, max 1000)" },
                    "id": { "type": "string", "description": "The record UUID (required for update/delete)" }
                },
                "required": ["entity", "action"]
            }),
        }
    }

    fn enriches_with_schema(&self) -> bool { true }

    async fn execute(&self, ctx: &ToolContext) -> Result<JsonValue, String> {
        let entity = str_arg(&ctx.args, "entity")?;
        let action = str_arg(&ctx.args, "action")?;
        let app = ctx.args.get("app").and_then(|v| v.as_str()).unwrap_or(&ctx.app_id);

        let perm_action = if action == "bulk_create" { "create" } else { action };
        check_permission(&ctx.permissions, &format!("app:{app}:{entity}.{perm_action}"))?;

        let tbl = table(app, entity);

        match action {
            "create" => {
                let obj = ctx.args.get("data").and_then(|v| v.as_object())
                    .ok_or("missing or invalid 'data' object")?;
                let types = field_type_map(&ctx.pool, app, entity).await.map_err(|e| e.to_string())?;

                let entries = filter_writable_fields(obj);
                if entries.is_empty() {
                    return Err("'data' must contain at least one writable field".into());
                }

                let cols: Vec<String> = entries.iter().map(|(k, _)| quote_ident(k)).collect();
                let phs: Vec<String> = (1..=entries.len()).map(|i| format!("${i}")).collect();

                let sql = format!(
                    "INSERT INTO {tbl} ({}) VALUES ({}) RETURNING to_jsonb({tbl}.*) AS row",
                    cols.join(", "), phs.join(", ")
                );
                let mut q = sqlx::query_as::<_, (JsonValue,)>(&sql);
                for (k, v) in &entries { q = bind_typed(q, v, types.get(*k)); }
                let (row,) = q.fetch_one(&ctx.pool).await.map_err(|e| e.to_string())?;
                Ok(row)
            }
            "update" => {
                let id = str_arg(&ctx.args, "id")?;
                let uuid: sqlx::types::Uuid = id.parse().map_err(|_| format!("invalid UUID: '{id}'"))?;
                let obj = ctx.args.get("data").and_then(|v| v.as_object())
                    .ok_or("missing or invalid 'data' object")?;
                let types = field_type_map(&ctx.pool, app, entity).await.map_err(|e| e.to_string())?;

                let entries = filter_writable_fields(obj);
                let id_param = entries.len() + 1;
                let mut sets: Vec<String> = entries.iter().enumerate()
                    .map(|(i, (k, _))| format!("{} = ${}", quote_ident(k), i + 1))
                    .collect();
                sets.push("\"updated_at\" = now()".to_string());

                let sql = format!(
                    "UPDATE {tbl} SET {} WHERE id = ${id_param} RETURNING to_jsonb({tbl}.*) AS row",
                    sets.join(", ")
                );
                let mut q = sqlx::query_as::<_, (JsonValue,)>(&sql);
                for (k, v) in &entries { q = bind_typed(q, v, types.get(*k)); }
                q = q.bind(uuid);
                let (row,) = q.fetch_optional(&ctx.pool).await.map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("record '{id}' not found"))?;
                Ok(row)
            }
            "bulk_create" => {
                let arr = ctx.args.get("data").and_then(|v| v.as_array())
                    .ok_or("'data' must be an array of objects for bulk_create")?;
                if arr.is_empty() {
                    return Err("'data' array must not be empty".into());
                }
                if arr.len() > MAX_BULK_SIZE {
                    return Err(format!("batch size {} exceeds max {MAX_BULK_SIZE}", arr.len()));
                }
                let objects: Vec<&serde_json::Map<String, JsonValue>> = arr.iter()
                    .map(|v| v.as_object().ok_or("each item in 'data' must be an object"))
                    .collect::<Result<_, _>>()?;
                let types = field_type_map(&ctx.pool, app, entity).await.map_err(|e| e.to_string())?;
                let rows = bulk_insert(&ctx.pool, &tbl, &types, &objects).await.map_err(|e| format!("{e:?}"))?;
                Ok(json!(rows))
            }
            "delete" => {
                let id = str_arg(&ctx.args, "id")?;
                let uuid: sqlx::types::Uuid = id.parse().map_err(|_| format!("invalid UUID: '{id}'"))?;
                let sql = format!("DELETE FROM {tbl} WHERE id = $1");
                let r = sqlx::query(&sql).bind(uuid).execute(&ctx.pool).await.map_err(|e| e.to_string())?;
                if r.rows_affected() == 0 { return Err(format!("record '{id}' not found")); }
                Ok(json!("Deleted successfully"))
            }
            _ => Err(format!("unknown action: '{action}'")),
        }
    }
}
