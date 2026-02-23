import { resolve } from "path";
import { access as fsAccess, constants } from "fs/promises";
import {
    AIMessage,
    HumanMessage,
    SystemMessage,
    type BaseMessage,
} from "@langchain/core/messages";
import { buildProvider } from "./provider.js";
import { buildDefaultGraph } from "./default-graph.js";
import { buildToolRegistry } from "./tools/registry.js";
import type { IpcWriter } from "./ipc.js";

export interface FieldSchema {
    name: string;
    type: string;
    required?: boolean;
    enumValues?: string[];
    references?: { entity: string; field: string };
}

export interface EntitySchema {
    entityName: string;
    fields: FieldSchema[];
}

export interface AgentConfig {
    model?: string;
    limits?: { maxTurns?: number };
    _appId: string;
    _agentId: string;
    _enabledTools: string[];
    _graphPath?: string;
    _dataContract?: EntitySchema[];
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

    const authToken = process.env.ROOTCX_AUTH_TOKEN ?? "";
    const agentId = config._agentId;

    const model = buildProvider(config.model);
    const tools = buildToolRegistry(config._enabledTools, {
        appId: config._appId,
        agentId,
        runtimeUrl,
        authToken,
        dataContract: config._dataContract ?? [],
    });

    const graph = await loadGraph(agentId, config._graphPath, model, tools);

    const messages: BaseMessage[] = [
        new SystemMessage(systemPrompt),
        ...deserializeHistory(history),
        new HumanMessage(message),
    ];

    const maxTurns = config.limits?.maxTurns ?? 10;
    let turns = 0;
    let lastResponse = "";

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
        if (!Array.isArray(agentMessages)) continue;

        for (const msg of agentMessages) {
            if (msg.content && typeof msg.content === "string") {
                lastResponse = msg.content;
                writer.send({
                    type: "agent_chunk",
                    session_id: sessionId,
                    delta: msg.content,
                });
            }
        }
    }

    writer.send({
        type: "agent_done",
        session_id: sessionId,
        response: lastResponse,
    });
}

async function loadGraph(
    agentId: string,
    graphPath: string | undefined,
    model: ReturnType<typeof buildProvider>,
    tools: ReturnType<typeof buildToolRegistry>,
) {
    // Try explicit graph path from manifest, then convention path
    const candidates = graphPath
        ? [resolve(graphPath)]
        : [resolve(`agents/${agentId}/graph.ts`), resolve(`agents/${agentId}/graph.js`)];

    for (const path of candidates) {
        if (await fileExists(path)) {
            const mod = await import(path);
            return typeof mod.default === "function"
                ? mod.default(model, tools)
                : mod.default;
        }
    }

    return buildDefaultGraph(model, tools);
}

async function fileExists(path: string): Promise<boolean> {
    try { await fsAccess(path, constants.R_OK); return true; } catch { return false; }
}

function deserializeHistory(history: Array<Record<string, unknown>>): BaseMessage[] {
    return history.map((msg) => {
        const content = String(msg.content ?? "");
        return msg.role === "user" ? new HumanMessage(content) : new AIMessage(content);
    });
}
