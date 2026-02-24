use crate::scaffold::emitter::Emitter;
use crate::scaffold::types::{Layer, LayerFuture, ScaffoldContext};

pub struct AgentLayer;

impl Layer for AgentLayer {
    fn emit<'a>(&'a self, ctx: &'a ScaffoldContext, e: &'a Emitter) -> LayerFuture<'a> {
        Box::pin(async move {
            let agent_runtime_dep = format!("file:{}", ctx.runtime.agent_runtime.display());

            let ai = ctx.ai_config.clone().unwrap_or_default();
            let provider = serde_json::to_value(ai.agent_provider_config())
                .map_err(|e| format!("failed to serialize provider config: {e}"))?;

            let manifest = serde_json::json!({
                "appId": ctx.app_id,
                "name": ctx.name,
                "version": "0.0.1",
                "description": "",
                "dataContract": [],
                "agent": {
                    "name": ctx.name,
                    "description": format!("AI agent for {}", ctx.name),
                    "provider": provider,
                    "systemPrompt": "./agent/system.md",
                    "memory": { "enabled": true },
                    "limits": { "maxTurns": 10 },
                    "access": []
                }
            });
            e.write_json("manifest.json", &manifest).await?;

            e.write("backend/agent/system.md", &format!(r#"You are the {} agent.

## Your role
Describe what this agent does.

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
