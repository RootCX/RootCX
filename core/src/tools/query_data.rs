use async_trait::async_trait;
use serde_json::{json, Value as JsonValue};
use rootcx_shared_types::ToolDescriptor;

use super::{Tool, ToolContext, str_arg};
use crate::manifest::field_type_map;
use crate::routes::crud::{
    build_where_clause, join_where, table, validate_order, validate_sort_field,
};

pub struct QueryDataTool;

#[async_trait]
impl Tool for QueryDataTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "query_data".into(),
            description: concat!(
                "Query records from a collection. Returns {data,total} with filters, or T[] for simple list.\n",
                "WHERE DSL (MongoDB-style):\n",
                "- Equality shorthand: {\"field\":\"value\"}\n",
                "- Operators: {\"field\":{\"$op\":value}} — $eq $ne $gt $gte $lt $lte $like $ilike $in $contains $isNull\n",
                "- $like/$ilike: SQL pattern (% = wildcard). $in: array. $contains: array subset. $isNull: bool.\n",
                "- Logic: $and:[...] $or:[...] $not:{...} — nestable. Top-level keys are AND-ed.\n",
                "Example: {\"$or\":[{\"status\":\"active\"},{\"role\":\"admin\"}],\"age\":{\"$gte\":18}}",
            ).into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "entity": { "type": "string", "description": "Collection/entity name" },
                    "app": { "type": "string", "description": "Target app ID for cross-app reads" },
                    "where": { "type": "object", "additionalProperties": true, "description": "WHERE clause — see DSL above" },
                    "orderBy": { "type": "string", "description": "Sort field (default: created_at)" },
                    "order": { "type": "string", "enum": ["asc", "desc"], "description": "Sort direction (default: desc)" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 1000, "description": "Max rows (default: 100)" },
                    "offset": { "type": "integer", "minimum": 0, "description": "Skip N rows (default: 0)" }
                },
                "required": ["entity"]
            }),
        }
    }

    fn enriches_with_schema(&self) -> bool { true }

    async fn execute(&self, ctx: &ToolContext) -> Result<JsonValue, String> {
        let entity = str_arg(&ctx.args, "entity")?;
        let app = ctx.args.get("app").and_then(|v| v.as_str()).unwrap_or(&ctx.app_id);

        let (_, perms) = crate::extensions::rbac::policy::resolve_permissions(&ctx.pool, app, ctx.user_id)
            .await.map_err(|e| format!("{e:?}"))?;
        let required = format!("{entity}.read");
        if !perms.iter().any(|p| p == "*" || *p == required) {
            return Err(format!("permission denied: {required}"));
        }

        let types = field_type_map(&ctx.pool, app, entity).await.map_err(|e| e.to_string())?;
        let tbl = table(app, entity);

        let query_keys = ["where", "orderBy", "order", "limit", "offset"];
        let has_query = query_keys.iter().any(|k| ctx.args.get(*k).is_some());

        if has_query {
            let (mut binds, mut idx) = (Vec::new(), 0usize);
            let mut conditions = Vec::new();

            if let Some(w) = ctx.args.get("where") {
                let sql = build_where_clause(w, &types, &mut binds, &mut idx)
                    .map_err(|e| format!("{e:?}"))?;
                if sql != "TRUE" { conditions.push(sql); }
            }

            let wh = join_where(&conditions);
            let sort = validate_sort_field(
                ctx.args.get("orderBy").and_then(|v| v.as_str()).map(String::from).as_ref(),
                &types,
            );
            let order = validate_order(
                ctx.args.get("order").and_then(|v| v.as_str()).map(String::from).as_ref(),
            );
            let limit = ctx.args.get("limit").and_then(|v| v.as_i64()).unwrap_or(100).min(1000).max(1);
            let offset = ctx.args.get("offset").and_then(|v| v.as_i64()).unwrap_or(0).max(0);

            let sql = format!(
                "SELECT to_jsonb(t.*) AS row, COUNT(*) OVER() AS total \
                 FROM {tbl} t{wh} ORDER BY {sort} {order} LIMIT {limit} OFFSET {offset}"
            );
            let mut q = sqlx::query_as::<_, (JsonValue, i64)>(&sql);
            for b in &binds { q = q.bind(b.as_str()); }
            let rows: Vec<(JsonValue, i64)> = q.fetch_all(&ctx.pool).await.map_err(|e| e.to_string())?;

            let total = rows.first().map(|(_, t)| *t).unwrap_or(0);
            let data: Vec<JsonValue> = rows.into_iter().map(|(r, _)| r).collect();
            Ok(json!({ "data": data, "total": total }))
        } else {
            let sql = format!(
                "SELECT to_jsonb(t.*) AS row FROM {tbl} t ORDER BY \"created_at\" DESC"
            );
            let rows: Vec<(JsonValue,)> = sqlx::query_as(&sql)
                .fetch_all(&ctx.pool).await.map_err(|e| e.to_string())?;
            Ok(JsonValue::Array(rows.into_iter().map(|(r,)| r).collect()))
        }
    }
}
