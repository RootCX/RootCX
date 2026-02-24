use std::collections::HashMap;

use crate::scaffold::emitter::Emitter;
use crate::scaffold::types::{AnswerValue, Layer, LayerFuture, ScaffoldContext};

pub struct AgentLayer;

fn provider_config(answers: &HashMap<String, AnswerValue>) -> serde_json::Value {
    let provider_type = match answers.get("provider") {
        Some(AnswerValue::Text(v)) => v.as_str(),
        _ => "anthropic",
    };
    match provider_type {
        "openai" => serde_json::json!({ "type": "openai", "model": "gpt-4o" }),
        "bedrock" => serde_json::json!({ "type": "bedrock", "model": "global.anthropic.claude-opus-4-6-v1" }),
        _ => serde_json::json!({ "type": "anthropic", "model": "claude-sonnet-4-20250514" }),
    }
}

impl Layer for AgentLayer {
    fn emit<'a>(&'a self, ctx: &'a ScaffoldContext, e: &'a Emitter) -> LayerFuture<'a> {
        Box::pin(async move {
            let agent_runtime_dep = format!("file:{}", ctx.runtime.agent_runtime.display());

            let manifest = serde_json::json!({
                "appId": ctx.app_id,
                "name": ctx.name,
                "version": "0.0.1",
                "description": "",
                "dataContract": [],
                "agent": {
                    "name": ctx.name,
                    "description": format!("AI agent for {}", ctx.name),
                    "provider": provider_config(&ctx.answers),
                    "systemPrompt": "./agent/system.md",
                    "memory": { "enabled": true },
                    "limits": { "maxTurns": 10 },
                    "access": []
                }
            });
            e.write_json("manifest.json", &manifest).await?;

            e.write("backend/agent/system.md", &format!(
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
"#, ctx.name)).await?;

            e.write("backend/agent/graph.ts", r#"import { StateGraph, MessagesAnnotation } from "@langchain/langgraph";
import { ToolNode } from "@langchain/langgraph/prebuilt";
import type { BaseChatModel } from "@langchain/core/language_models/chat_models";
import type { StructuredToolInterface } from "@langchain/core/tools";

export default function buildGraph(model: BaseChatModel, tools: StructuredToolInterface[]) {
    const bound = model.bindTools(tools);
    const toolNode = new ToolNode(tools);

    async function agent(state: typeof MessagesAnnotation.State) {
        return { messages: [await bound.invoke(state.messages)] };
    }

    function route(state: typeof MessagesAnnotation.State) {
        const last = state.messages.at(-1) as { tool_calls?: unknown[] } | undefined;
        return last?.tool_calls?.length ? "tools" : "__end__";
    }

    return new StateGraph(MessagesAnnotation)
        .addNode("agent", agent)
        .addNode("tools", toolNode)
        .addEdge("__start__", "agent")
        .addConditionalEdges("agent", route)
        .addEdge("tools", "agent")
        .compile();
}
"#).await?;

            e.write("backend/index.ts", "import \"@rootcx/agent-runtime\";\n").await?;

            e.write("backend/package.json", &format!(
                r#"{{"name":"{}","version":"0.1.0","private":true,"type":"module","dependencies":{{"@rootcx/agent-runtime":"{agent_runtime_dep}"}}}}"#,
                ctx.app_id
            )).await?;

            e.write(".rootcx/launch.json",
                "{\n  \"preLaunch\": [\"verify_schema\", \"sync_manifest\", \"deploy_backend\"]\n}\n"
            ).await?;

            Ok(())
        })
    }
}
