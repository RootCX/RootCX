use crate::scaffold::emitter::Emitter;
use crate::scaffold::types::{Layer, LayerFuture, ScaffoldContext};

pub struct AgentLayer {
    pub agent_id: String,
}

impl Layer for AgentLayer {
    fn emit<'a>(&'a self, ctx: &'a ScaffoldContext, e: &'a Emitter) -> LayerFuture<'a> {
        let agent_id = self.agent_id.clone();
        Box::pin(async move {
            let agent_runtime_dep = format!("file:{}", ctx.runtime.agent_runtime.display());

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
                        "limits": { "maxTurns": 10 },
                        "access": []
                    }
                }
            });
            e.write_json("manifest.json", &manifest).await?;

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

            e.write(
                &format!("agents/{agent_id}/graph.ts"),
                r#"import { StateGraph, MessagesAnnotation, Annotation } from "@langchain/langgraph";
import { ToolNode } from "@langchain/langgraph/prebuilt";
import { HumanMessage, SystemMessage } from "@langchain/core/messages";
import type { BaseChatModel } from "@langchain/core/language_models/chat_models";
import type { StructuredToolInterface } from "@langchain/core/tools";

const PHASES = ["research", "execute"] as const;
type Phase = (typeof PHASES)[number];

const PHASE_PROMPT: Record<Phase, string> = {
    research: "PHASE: RESEARCH — Gather information using available tools. Do not take actions yet. Summarize findings when done.",
    execute: "PHASE: EXECUTE — Based on your research, take action: create/update records, produce outputs, and report results.",
};

const State = Annotation.Root({
    ...MessagesAnnotation.spec,
    phase: Annotation<Phase>({ reducer: (_, b) => b, default: () => PHASES[0] }),
});

export default function buildGraph(model: BaseChatModel, tools: StructuredToolInterface[]) {
    const boundModel = model.bindTools(tools);
    const toolNode = new ToolNode(tools);

    async function agent(state: typeof State.State) {
        const response = await boundModel.invoke([
            new SystemMessage(PHASE_PROMPT[state.phase]),
            ...state.messages,
        ]);
        return { messages: [response] };
    }

    function route(state: typeof State.State) {
        const last = state.messages.at(-1) as any;
        if (last?.tool_calls?.length) return "tools";
        if (state.phase !== PHASES.at(-1)) return "next_phase";
        return "__end__";
    }

    function nextPhase(state: typeof State.State) {
        const next = PHASES[PHASES.indexOf(state.phase) + 1];
        return { phase: next, messages: [new HumanMessage(`Proceed to phase: ${next}`)] };
    }

    return new StateGraph(State)
        .addNode("agent", agent)
        .addNode("tools", toolNode)
        .addNode("next_phase", nextPhase)
        .addEdge("__start__", "agent")
        .addConditionalEdges("agent", route)
        .addEdge("tools", "agent")
        .addEdge("next_phase", "agent")
        .compile();
}
"#,
            )
            .await?;

            e.write("index.ts", "import \"@rootcx/agent-runtime\";\n")
                .await?;

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

            e.write(
                ".rootcx/launch.json",
                "{\n  \"preLaunch\": [\"verify_schema\", \"sync_manifest\", \"deploy_backend\"]\n}\n",
            )
            .await?;

            Ok(())
        })
    }
}
