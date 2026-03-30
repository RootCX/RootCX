import { initChatModel } from "langchain/chat_models/universal";
import { createAgent, tool, modelRetryMiddleware, modelCallLimitMiddleware, toolRetryMiddleware } from "langchain";
import { createInterface } from "readline";
import { readFileSync } from "fs";
import { z } from "zod";

const TOOL_TIMEOUT_MS = 60_000;
const write = (m: any) => process.stdout.write(JSON.stringify(m) + "\n");
const rl = createInterface({ input: process.stdin });
const calls = new Map<string, { resolve: (v: string) => void; timer: ReturnType<typeof setTimeout> }>();

let agentConfig: { tools: any[]; systemPrompt: string; maxTurns: number } | null = null;
let cachedModelKey = "";
let cachedAgent: any = null;

rl.on("line", (l) => {
    let m: any;
    try { m = JSON.parse(l); } catch { return; }
    if (m.type === "discover") { boot(m); return; }
    if (m.type === "agent_tool_result") {
        const pending = calls.get(m.call_id);
        if (pending) {
            clearTimeout(pending.timer);
            calls.delete(m.call_id);
            pending.resolve(JSON.stringify(m.error ? { error: m.error } : m.result));
        }
        return;
    }
    if (m.type === "agent_invoke") invoke(m);
});

write({ type: "discover", capabilities: ["agent"] });

function boot(m: any) {
    const cfg = m.agent_config;
    if (!cfg) return;
    const credentials = m.credentials ?? {};
    for (const [k, v] of Object.entries(credentials)) process.env[k] = v as string;

    let systemPrompt = "";
    try { systemPrompt = readFileSync("./agent/system.md", "utf-8"); } catch {}

    const tools = (cfg.tool_descriptors ?? []).map((t: any) =>
        tool(
            (args: any, config: any) => new Promise<string>((resolve) => {
                const invokeId = config?.configurable?.invokeId ?? "";
                const id = crypto.randomUUID();
                const timer = setTimeout(() => {
                    calls.delete(id);
                    resolve(JSON.stringify({ error: "tool call timed out" }));
                }, TOOL_TIMEOUT_MS);
                calls.set(id, { resolve, timer });
                write({ type: "agent_tool_call", invoke_id: invokeId, call_id: id, tool_name: t.name, args });
            }),
            { name: t.name, description: t.description, schema: toZod(t.inputSchema) },
        )
    );

    agentConfig = { tools, systemPrompt, maxTurns: cfg.max_turns ?? 50 };
}

async function invoke(m: any) {
    if (!agentConfig || !m.invoke_id || !m.message) {
        write({ type: "agent_error", invoke_id: m.invoke_id ?? "", error: "agent not ready or missing fields" });
        return;
    }

    try {
        const llm = m.llm;
        if (!llm) {
            write({ type: "agent_error", invoke_id: m.invoke_id, error: "no llm model in invoke payload" });
            return;
        }

        const modelKey = `${llm.provider}:${llm.model}`;
        if (modelKey !== cachedModelKey || !cachedAgent) {
            const model = await initChatModel(modelKey);
            cachedAgent = createAgent({
                model,
                tools: agentConfig.tools,
                systemPrompt: agentConfig.systemPrompt,
                middleware: [
                    modelRetryMiddleware({ maxRetries: 3, backoffFactor: 2, initialDelayMs: 1000 }),
                    modelCallLimitMiddleware({ runLimit: agentConfig.maxTurns }),
                    toolRetryMiddleware({ maxRetries: 3, onFailure: "continue" }),
                ],
            });
            cachedModelKey = modelKey;
        }

        let response = "";
        const stream = await cachedAgent.stream(
            { messages: [...(m.history ?? []), { role: "user", content: m.message }] },
            { streamMode: "messages" as const, recursionLimit: 150, configurable: { invokeId: m.invoke_id } },
        );
        for await (const [chunk, metadata] of stream) {
            if (metadata.langgraph_node !== "model_request") continue;
            const text = typeof chunk.content === "string" ? chunk.content : "";
            if (text) { response += text; write({ type: "agent_chunk", invoke_id: m.invoke_id, delta: text }); }
        }
        write({ type: "agent_done", invoke_id: m.invoke_id, response });
    } catch (e: any) {
        write({ type: "agent_error", invoke_id: m.invoke_id, error: e.message ?? String(e) });
    }
}

function toZod(s: any): z.ZodObject<any> {
    if (s?.type !== "object") return z.object({}).passthrough();
    const shape: Record<string, z.ZodTypeAny> = {};
    for (const [k, v] of Object.entries(s.properties ?? {}) as [string, any][]) {
        let f: z.ZodTypeAny =
            v.type === "string" ? z.string() :
            v.type === "number" || v.type === "integer" ? z.number() :
            v.type === "boolean" ? z.boolean() :
            v.type === "array" ? z.array(z.any()) :
            z.any();
        if (v.description) f = f.describe(v.description);
        shape[k] = s.required?.includes(k) ? f : f.optional();
    }
    return z.object(shape);
}
