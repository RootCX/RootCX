use async_trait::async_trait;
use rootcx_types::ToolDescriptor;
use serde_json::{Value as JsonValue, json};
use std::collections::HashMap;

use super::{Tool, ToolContext};

pub struct ListAppsTool;

#[async_trait]
impl Tool for ListAppsTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "list_apps".into(),
            description: "List accessible installed apps and their readable entity names.".into(),
            input_schema: json!({ "type": "object", "properties": {} }),
        }
    }

    async fn execute(&self, ctx: &ToolContext) -> Result<JsonValue, String> {
        let source = ctx
            .core_bound_app_id
            .as_deref()
            .filter(|source| *source == ctx.app_id);
        let grants: Vec<(String, String, Vec<String>)> = if let Some(source) = source {
            sqlx::query_as(
                "SELECT g.provider_app, g.entity, g.field_snapshot
                   FROM rootcx_system.cross_app_collection_grants g
                   JOIN rootcx_system.app_installations ci ON ci.id = g.consumer_installation_id
                   JOIN rootcx_system.app_installations pi ON pi.id = g.provider_installation_id
                   JOIN rootcx_system.apps ca ON ca.id = g.consumer_app
                   JOIN rootcx_system.apps pa ON pa.id = g.provider_app
                  WHERE g.consumer_app = $1 AND g.status = 'active'
                    AND (g.expires_at IS NULL OR g.expires_at > now())
                    AND ci.active = TRUE AND pi.active = TRUE
                    AND ca.status IN ('installed', 'system')
                    AND pa.status IN ('installed', 'system')
                  ORDER BY g.provider_app, g.entity",
            )
            .bind(source)
            .fetch_all(&ctx.pool)
            .await
            .map_err(|_| "available apps could not be loaded".to_string())?
        } else {
            Vec::new()
        };
        let mut granted: HashMap<String, Vec<(String, Vec<String>)>> = HashMap::new();
        for (provider, entity, fields) in grants {
            granted.entry(provider).or_default().push((entity, fields));
        }
        let rows: Vec<(String, String, JsonValue)> = sqlx::query_as(
            "SELECT id, name, COALESCE(manifest->'dataContract', '[]'::jsonb) \
             FROM rootcx_system.apps WHERE status = 'installed' ORDER BY name",
        )
        .fetch_all(&ctx.pool)
        .await
        .map_err(|e| e.to_string())?;

        Ok(rows
            .into_iter()
            .filter_map(|(id, name, dc)| {
                let projected = match source {
                    Some(source) if source == id => dc,
                    Some(_) => {
                        let scopes = granted.get(&id)?;
                        crate::tools::describe_app::project_contract(dc, scopes)
                    }
                    None => crate::tools::describe_app::project_human_contract(
                        dc,
                        &id,
                        &ctx.permissions,
                    ),
                };
                let entities: Vec<&str> = projected
                    .as_array()?
                    .iter()
                    .filter_map(|e| e.get("entityName").and_then(|v| v.as_str()))
                    .collect();
                if entities.is_empty() {
                    return None;
                }
                Some(json!({ "id": id, "name": name, "entities": entities }))
            })
            .collect())
    }
}
