use async_trait::async_trait;
use rootcx_types::ToolDescriptor;
use serde_json::{Value as JsonValue, json};
use std::collections::HashSet;

use super::{Tool, ToolContext, str_arg};

pub struct DescribeAppTool;

pub(crate) fn project_contract(contract: JsonValue, grants: &[(String, Vec<String>)]) -> JsonValue {
    let allowed: std::collections::HashMap<&str, HashSet<&str>> = grants
        .iter()
        .map(|(entity, fields)| (entity.as_str(), fields.iter().map(String::as_str).collect()))
        .collect();
    JsonValue::Array(
        contract
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|entity| {
                let name = entity.get("entityName")?.as_str()?;
                let fields = allowed.get(name)?;
                // Build an allowlist rather than copying manifest objects:
                // defaults, links, checks and indexes can disclose fields and
                // values outside the approved collection projection.
                let fields: Vec<JsonValue> = entity
                    .get("fields")?
                    .as_array()?
                    .iter()
                    .filter_map(|field| {
                        let name = field.get("name")?.as_str()?;
                        if !fields.contains(name)
                            || field.get("sensitive").and_then(JsonValue::as_bool) == Some(true)
                        {
                            return None;
                        }
                        let field_type = field.get("type")?.as_str()?;
                        Some(json!({ "name": name, "type": field_type }))
                    })
                    .collect();
                Some(json!({ "entityName": name, "fields": fields }))
            })
            .collect(),
    )
}

/// Human HTTP contexts have no authenticated application source. Their entity
/// visibility comes exclusively from resolved user permissions, never appId.
pub(crate) fn project_human_contract(
    contract: JsonValue,
    app: &str,
    permissions: &[String],
) -> JsonValue {
    let scopes = contract
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entity| {
            let name = entity.get("entityName")?.as_str()?;
            let read = format!("app:{app}:{name}.read");
            if !crate::governance::authority::has_permission(permissions, &read)
                && !crate::governance::authority::has_permission(
                    permissions,
                    &format!("{read}.own"),
                )
                && !crate::governance::authority::has_permission(
                    permissions,
                    &format!("{read}.shared"),
                )
            {
                return None;
            }
            let fields = entity
                .get("fields")?
                .as_array()?
                .iter()
                .filter_map(|field| field.get("name")?.as_str().map(String::from))
                .collect();
            Some((name.to_string(), fields))
        })
        .collect::<Vec<_>>();
    project_contract(contract, &scopes)
}

#[async_trait]
impl Tool for DescribeAppTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "describe_app".into(),
            description: "Get readable entity and field metadata for an app. Core-bound workers receive the full contract for their own app.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "app": { "type": "string", "description": "The app ID to describe" }
                },
                "required": ["app"]
            }),
        }
    }

    async fn execute(&self, ctx: &ToolContext) -> Result<JsonValue, String> {
        let app = str_arg(&ctx.args, "app")?;
        let source = ctx
            .core_bound_app_id
            .as_deref()
            .filter(|source| *source == ctx.app_id);
        let grants: Vec<(String, Vec<String>)> =
            if let Some(source) = source.filter(|source| *source != app) {
                sqlx::query_as(
                    "SELECT g.entity, g.field_snapshot
                   FROM rootcx_system.cross_app_collection_grants g
                   JOIN rootcx_system.app_installations ci ON ci.id = g.consumer_installation_id
                   JOIN rootcx_system.app_installations pi ON pi.id = g.provider_installation_id
                   JOIN rootcx_system.apps ca ON ca.id = g.consumer_app
                   JOIN rootcx_system.apps pa ON pa.id = g.provider_app
                  WHERE g.consumer_app = $1 AND g.provider_app = $2
                    AND g.status = 'active'
                    AND (g.expires_at IS NULL OR g.expires_at > now())
                    AND ci.active = TRUE AND pi.active = TRUE
                    AND ca.status IN ('installed', 'system')
                    AND pa.status IN ('installed', 'system')
                  ORDER BY g.entity",
                )
                .bind(source)
                .bind(app)
                .fetch_all(&ctx.pool)
                .await
                .map_err(|_| "app is not available".to_string())?
            } else {
                Vec::new()
            };
        if source.is_some_and(|source| source != app) && grants.is_empty() {
            return Err(format!("app '{app}' not found"));
        }
        let (name, dc): (String, JsonValue) = sqlx::query_as(
            "SELECT name, COALESCE(manifest->'dataContract', '[]'::jsonb) \
             FROM rootcx_system.apps WHERE id = $1 AND status IN ('installed', 'system')",
        )
        .bind(app)
        .fetch_optional(&ctx.pool)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("app '{app}' not found"))?;

        let dc = match source {
            Some(source) if source == app => dc,
            Some(_) => project_contract(dc, &grants),
            None => project_human_contract(dc, app, &ctx.permissions),
        };
        if dc.as_array().is_none_or(Vec::is_empty) {
            return Err(format!("app '{app}' not found"));
        }
        Ok(json!({ "app": app, "name": name, "dataContract": dc }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_metadata_exposes_only_granted_non_sensitive_names_and_types() {
        let contract = json!([{
            "entityName": "records",
            "identityKey": "secret",
            "indexes": [{"columns": ["secret"], "where": "secret = 'private'"}],
            "checks": [{"expr": "secret <> 'private'"}],
            "futureAttribute": "private",
            "fields": [
                {
                    "name": "name", "type": "text", "required": true,
                    "default_value": "private", "enum_values": ["private"],
                    "references": {"app": "hidden", "entity": "secrets"},
                    "futureAttribute": "private"
                },
                {"name": "secret", "type": "text", "sensitive": true},
                {"name": "unapproved", "type": "text"}
            ]
        }, {
            "entityName": "hidden", "fields": [{"name": "secret", "type": "text"}]
        }]);
        assert_eq!(
            project_contract(
                contract,
                &[("records".into(), vec!["name".into(), "secret".into()])]
            ),
            json!([{"entityName": "records", "fields": [{"name": "name", "type": "text"}]}]),
        );
    }
}
