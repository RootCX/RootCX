use crate::scaffold::emitter::Emitter;
use crate::scaffold::types::{Layer, LayerFuture, ScaffoldContext};

/// Emits a complete agent-only project: manifest.json with agents section,
/// agents/{id}/system.md, package.json with agent-runtime, and launch config.
/// No frontend, no React, no Tauri.
pub struct AgentLayer {
    pub agent_id: String,
}

impl Layer for AgentLayer {
    fn emit<'a>(&'a self, ctx: &'a ScaffoldContext, e: &'a Emitter) -> LayerFuture<'a> {
        let agent_id = self.agent_id.clone();
        Box::pin(async move {
            let agent_runtime_dep = format!("file:{}", ctx.runtime.agent_runtime.display());

            // manifest.json with agents section
            let manifest = serde_json::json!({
                "appId": ctx.app_id,
                "name": ctx.name,
                "version": "0.0.1",
                "description": "",
                "dataContract": [],
                "agents": {
                    &agent_id: {
                        "name": ctx.name,
                        "description": format!("AI agent for {}", ctx.name),
                        "model": "global.anthropic.claude-opus-4-6-v1",
                        "systemPrompt": format!("./agents/{agent_id}/system.md"),
                        "memory": { "enabled": true },
                        "limits": { "maxTurns": 10, "maxBudgetUsd": 1.0 },
                        "access": []
                    }
                }
            });
            e.write_json("manifest.json", &manifest).await?;

            // System prompt template
            let system_prompt = format!(
                r#"You are the {} agent.

## Your role
Describe what this agent does.

## Data you work with
List the entities from your dataContract here.

## Workflow
1. Step one
2. Step two
3. Step three

## Rules
- Be specific about constraints
- Reference entity names from the manifest
"#,
                ctx.name
            );
            e.write(
                &format!("agents/{agent_id}/system.md"),
                &system_prompt,
            )
            .await?;

            // package.json — agent-only, no frontend deps
            e.write(
                "package.json",
                &format!(
                    r#"{{
  "name": "{}",
  "version": "0.1.0",
  "private": true,
  "type": "module",
  "dependencies": {{
    "@rootcx/agent-runtime": "{agent_runtime_dep}"
  }}
}}
"#,
                    ctx.app_id
                ),
            )
            .await?;

            // Launch config for agent projects — no local command needed,
            // the agent worker runs inside Core after deploy.
            e.write(
                ".rootcx/launch.json",
                "{\n  \"preLaunch\": [\"verify_schema\", \"sync_manifest\", \"deploy_backend\"]\n}\n",
            )
            .await?;

            Ok(())
        })
    }
}
