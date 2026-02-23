import {
    AIMessage,
    HumanMessage,
    SystemMessage,
    type BaseMessage,
} from "@langchain/core/messages";
import { buildDefaultGraph } from "./default-graph.js";
import { buildProvider } from "./provider.js";
import { buildToolRegistry } from "./tools/registry.js";
import type { IpcWriter } from "./ipc.js";

export interface AgentConfig {
    model?: string;
    limits?: { maxTurns?: number; maxBudgetUsd?: number };
    _appId: string;
    _enabledTools: string[];
    _graphAbsolutePath?: string;
}

interface RunAgentParams {
    sessionId: string;
    message: string;
    systemPrompt: string;
    config: AgentConfig;
    history: Array<Record<string, unknown>>;
    writer: IpcWriter;
}

export async function runAgent(params: RunAgentParams) {
    const { sessionId, message, systemPrompt, config, history, writer } = params;

    const runtimeUrl = process.env.ROOTCX_RUNTIME_URL;
    if (!runtimeUrl) throw new Error("ROOTCX_RUNTIME_URL not set");

    const model = buildProvider(config.model);
    const tools = buildToolRegistry(config._enabledTools, {
        appId: config._appId,
        agentId: config._appId,
        runtimeUrl,
    });

    let graph;
    if (config._graphAbsolutePath) {
        const custom = await import(config._graphAbsolutePath);
        graph = typeof custom.default === "function"
            ? custom.default(model, tools)
            : custom.default;
    } else {
        graph = buildDefaultGraph(model, tools);
    }

    const messages: BaseMessage[] = [
        new SystemMessage(systemPrompt),
        ...deserializeHistory(history),
        new HumanMessage(message),
    ];

    const maxTurns = config.limits?.maxTurns ?? 10;
    let turns = 0;
    let finalResponse = "";

    const stream = await graph.stream(
        { messages },
        { recursionLimit: maxTurns * 2 },
    );

    for await (const event of stream) {
        if (++turns > maxTurns) {
            writer.send({
                type: "agent_error",
                session_id: sessionId,
                error: `Max turns (${maxTurns}) exceeded`,
            });
            return;
        }

        const agentMessages = event.agent?.messages ?? event.messages;
        if (agentMessages && Array.isArray(agentMessages)) {
            for (const msg of agentMessages) {
                if (msg.content && typeof msg.content === "string") {
                    finalResponse = msg.content;
                    writer.send({
                        type: "agent_chunk",
                        session_id: sessionId,
                        delta: msg.content,
                    });
                }
            }
        }
    }

    writer.send({
        type: "agent_done",
        session_id: sessionId,
        response: finalResponse,
        tokens: 0,
    });
}

function deserializeHistory(
    history: Array<Record<string, unknown>>,
): BaseMessage[] {
    return history.map((msg) => {
        const content = String(msg.content ?? "");
        if (msg.role === "user") return new HumanMessage(content);
        return new AIMessage(content);
    });
}
