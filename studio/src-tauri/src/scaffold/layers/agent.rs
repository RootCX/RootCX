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
                "name": ctx.app_id,
                "version": "0.0.1",
                "description": "",
                "dataContract": [
                    {
                        "entityName": "agent_tasks",
                        "fields": [
                            { "name": "title", "type": "text", "required": true },
                            { "name": "status", "type": "text", "required": true, "enumValues": ["pending", "in_progress", "completed", "failed"] },
                            { "name": "input", "type": "json" },
                            { "name": "result", "type": "text" }
                        ]
                    }
                ],
                "agent": {
                    "name": ctx.app_id,
                    "description": format!("AI agent for {}", ctx.app_id),
                    "provider": provider,
                    "systemPrompt": "./agent/system.md",
                    "memory": { "enabled": true },
                    "limits": {
                        "maxTurns": 50,
                        "maxContextTokens": 100000,
                        "keepRecentMessages": 10
                    },
                    "supervision": {
                        "mode": "autonomous"
                    },
                    "access": [
                        { "entity": "agent_tasks", "actions": ["create", "read", "update", "delete"] }
                    ]
                }
            });
            e.write_json("manifest.json", &manifest).await?;

            e.write("backend/agent/system.md", &format!(r#"You are the {} agent.

## Your role
Describe what this agent does.

## Data
You have access to the agent_tasks entity to track your work:
- title (text, required): Task description
- status (text, required): pending | in_progress | completed | failed
- input (json): Task input data
- result (text): Task output

## Workflow
1. Receive a request
2. Create an AgentTask to track it
3. Process the request
4. Update the task with the result

## Rules
- Always create a task before starting work
- Update task status as you progress
- Store results in the task record
"#, ctx.app_id)).await?;

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
