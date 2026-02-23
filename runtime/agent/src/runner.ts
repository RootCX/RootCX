import { resolve } from "path";
import { access as fsAccess, constants } from "fs/promises";
import {
    AIMessage,
    HumanMessage,
    SystemMessage,
    type BaseMessage,
} from "@langchain/core/messages";
import type { BaseChatModel } from "@langchain/core/language_models/chat_models";
import type { StructuredToolInterface } from "@langchain/core/tools";
import { buildProvider, type ProviderConfig } from "./providers/index.js";
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
    provider: ProviderConfig;
    limits?: { maxTurns?: number };
    _appId: string;
    _enabledTools: string[];
    _graphPath?: string;
    _dataContract?: EntitySchema[];
}

interface RunAgentParams {
    invokeId: string;
    message: string;
    systemPrompt: string;
    config: AgentConfig;
    authToken: string;
    history: Array<Record<string, unknown>>;
    writer: IpcWriter;
}

export async function runAgent(params: RunAgentParams) {
    const { invokeId, message, systemPrompt, config, authToken, history, writer } = params;

    const runtimeUrl = process.env.ROOTCX_RUNTIME_URL;
    if (!runtimeUrl) throw new Error("ROOTCX_RUNTIME_URL not set");

    const model = await buildProvider(config.provider);
    const tools = buildToolRegistry(config._enabledTools, {
        appId: config._appId,
        runtimeUrl,
        authToken,
        dataContract: config._dataContract ?? [],
    });

    const graph = await loadGraph(config._graphPath, model, tools);

    const messages: BaseMessage[] = [
        new SystemMessage(systemPrompt),
        ...deserializeHistory(history),
        new HumanMessage(message),
    ];

    const maxTurns = config.limits?.maxTurns ?? 10;
    let turns = 0;
    let fullResponse = "";

    const stream = graph.streamEvents(
        { messages },
        { version: "v2", recursionLimit: maxTurns * 2 },
    );

    for await (const event of stream) {
        if (event.event === "on_chat_model_stream") {
            const chunk = event.data?.chunk;
            const delta = typeof chunk?.content === "string" ? chunk.content : "";
            if (delta) {
                fullResponse += delta;
                writer.send({
                    type: "agent_chunk",
                    invoke_id: invokeId,
                    delta,
                });
            }
        } else if (event.event === "on_chat_model_end") {
            if (++turns > maxTurns) {
                writer.send({
                    type: "agent_error",
                    invoke_id: invokeId,
                    error: `Max turns (${maxTurns}) exceeded`,
                });
                return;
            }
        }
    }

    writer.send({
        type: "agent_done",
        invoke_id: invokeId,
        response: fullResponse,
    });
}

async function importGraph(
    path: string,
    model: BaseChatModel,
    tools: StructuredToolInterface[],
) {
    const mod = await import(path);
    return typeof mod.default === "function"
        ? mod.default(model, tools)
        : mod.default;
}

async function loadGraph(
    graphPath: string | undefined,
    model: BaseChatModel,
    tools: StructuredToolInterface[],
) {
    if (graphPath) {
        const resolved = resolve(graphPath);
        if (!await fileExists(resolved)) {
            throw new Error(`graph file not found: ${resolved}`);
        }
        return importGraph(resolved, model, tools);
    }

    for (const path of [resolve("agent/graph.ts"), resolve("agent/graph.js")]) {
        if (await fileExists(path)) {
            return importGraph(path, model, tools);
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
