use async_trait::async_trait;
use rootcx_types::ToolDescriptor;
use serde_json::{Value as JsonValue, json};

use super::{Tool, ToolContext, str_arg};
use crate::data_types::row_json;
use crate::governance::collection_reads::{self, EqualityMode, PAGE_OPTION_KEYS};
use crate::governance::cross_app;
use crate::governance::cross_app_operations::{self, OperationContext};
use crate::governance::enforcement::{self, ContextState};
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

    fn enriches_with_schema(&self) -> bool {
        true
    }

    // A read fetches the full set in one query; mapping it over N upstream items
    // would re-run the same query N times for nothing.
    fn batch_mode(&self) -> super::BatchMode {
        super::BatchMode::Once
    }

    async fn execute(&self, ctx: &ToolContext) -> Result<JsonValue, String> {
        let entity = str_arg(&ctx.args, "entity")?;
        let app = ctx
            .args
            .get("app")
            .and_then(|v| v.as_str())
            .unwrap_or(&ctx.app_id);

        // The owner pool bypasses RLS until the transaction installs the role
        // and responsible human, constrained by the caller's permission ceiling.
        let state = ContextState {
            user_id: ctx.data_user_id(),
            is_delegated: true,
            effective_perms: ctx.permissions.clone(),
            connection_id: None,
            audit_actor_id: Some(ctx.user_id),
            audit_delegator_id: ctx.invoker_user_id,
            public_execution: None,
        };
        let has_query = PAGE_OPTION_KEYS
            .iter()
            .any(|key| ctx.args.get(*key).is_some());
        if app != ctx.app_id {
            let Some(source_app) = ctx
                .core_bound_app_id
                .as_deref()
                .filter(|bound| *bound == ctx.app_id)
            else {
                // Keep the attempted target visible without recording the
                // caller-controlled HTTP app id as an authenticated source.
                if cross_app::record_read_audit(
                    &ctx.pool,
                    None,
                    "unbound",
                    app,
                    entity,
                    cross_app::ACTION_LIST,
                    Some(ctx.user_id),
                    ctx.invoker_user_id,
                    "denied",
                    Some("unbound_context"),
                    None,
                    uuid::Uuid::new_v4(),
                )
                .await
                .is_err()
                {
                    return Err("cross-app audit unavailable".into());
                }
                return Err("cross-app access requires a Core-bound application worker".into());
            };
            let options: serde_json::Map<String, JsonValue> = PAGE_OPTION_KEYS
                .iter()
                .filter_map(|key| {
                    ctx.args
                        .get(*key)
                        .map(|value| ((*key).into(), value.clone()))
                })
                .collect();
            return cross_app_operations::execute(
                &ctx.pool,
                source_app,
                app,
                if has_query { "findPage" } else { "findAll" },
                entity,
                JsonValue::Object(options),
                OperationContext::query_tool(state),
            )
            .await;
        }

        let types = field_type_map(&ctx.pool, app, entity)
            .await
            .map_err(|error| error.to_string())?;
        let mut tx = enforcement::begin_app_tx(
            &ctx.pool,
            app,
            &state,
            Some(ctx.user_id),
            ctx.invoker_user_id,
            "agent_tool",
            enforcement::TIMEOUT_AGENT_TOOL_MS,
        )
        .await
        .map_err(|error| error.to_string())?;

        let tbl = table(app, entity);
        // Keep the legacy local tool's permissive pagination normalization.
        // Explicit worker findPage and remote page requests use the strict reader.
        let query_result: Result<JsonValue, String> = async {
            if has_query {
                let (mut binds, mut idx) = (Vec::new(), 0usize);
                let mut conditions = Vec::new();

                if let Some(w) = ctx.args.get("where") {
                    let sql = build_where_clause(w, &types, &mut binds, &mut idx)
                        .map_err(|e| format!("{e:?}"))?;
                    if sql != "TRUE" {
                        conditions.push(sql);
                    }
                }

                let wh = join_where(&conditions);
                let order_by = ctx
                    .args
                    .get("orderBy")
                    .and_then(|v| v.as_str())
                    .map(String::from);
                let order = validate_order(
                    ctx.args
                        .get("order")
                        .and_then(|v| v.as_str())
                        .map(String::from)
                        .as_ref(),
                );
                let limit = ctx
                    .args
                    .get("limit")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(100)
                    .clamp(1, 1000);
                let offset = ctx
                    .args
                    .get("offset")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0)
                    .max(0);
                let sort = validate_sort_field(order_by.as_ref(), &types);

                let row = row_json("t", &types);
                let sql = format!(
                    "SELECT {row} AS row, COUNT(*) OVER() AS total \
                 FROM {tbl} t{wh} ORDER BY {sort} {order} LIMIT {limit} OFFSET {offset}"
                );
                let mut q = sqlx::query_as::<_, (JsonValue, i64)>(&sql);
                for b in &binds {
                    q = q.bind(b.as_str());
                }
                let rows: Vec<(JsonValue, i64)> =
                    q.fetch_all(&mut *tx).await.map_err(|e| e.to_string())?;

                let total = rows.first().map(|(_, t)| *t).unwrap_or(0);
                let data: Vec<JsonValue> = rows.into_iter().map(|(r, _)| r).collect();
                Ok(json!({ "data": data, "total": total }))
            } else {
                collection_reads::equality(
                    &mut tx,
                    &types,
                    app,
                    entity,
                    json!({}),
                    EqualityMode::NewestFirst,
                )
                .await
            }
        }
        .await;
        let result = match query_result {
            Ok(result) => result,
            Err(error) => {
                let _ = tx.rollback().await;
                return Err(error);
            }
        };
        tx.commit()
            .await
            .map_err(|_| "collection query unavailable".to_string())?;
        Ok(result)
    }
}
