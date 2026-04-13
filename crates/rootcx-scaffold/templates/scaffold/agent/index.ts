__LLM_IMPORT__
import { createAgent, tool, modelRetryMiddleware, modelCallLimitMiddleware, toolRetryMiddleware } from "langchain";
import { createInterface } from "readline";
import { readFileSync } from "fs";
import { z } from "zod";

const TOOL_TIMEOUT_MS = 60_000;
const write = (m: any) => process.stdout.write(JSON.stringify(m) + "\n");
const rl = createInterface({ input: process.stdin });
const calls = new Map<string, { resolve: (v: string) => void; timer: ReturnType<typeof setTimeout> }>();

let agent: any = null;

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
    if (m.type === "job") invokeJob(m);
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

    const maxTurns = cfg.max_turns ?? 50;

    agent = createAgent({
        model: __LLM_INIT__,
        tools,
        systemPrompt,
        middleware: [
            modelRetryMiddleware({ maxRetries: 3, backoffFactor: 2, initialDelayMs: 1000 }),
            modelCallLimitMiddleware({ runLimit: maxTurns }),
            toolRetryMiddleware({ maxRetries: 3, onFailure: "continue" }),
        ],
    });
}

// --- Multimodal helpers ---

// Attachment carries a one-time nonce URL — bytes are fetched via HTTP, not passed in IPC.
type Attachment = { name: string; content_type: string; url: string };
// Fetched attachment: bytes loaded once, reused across rich and fallback paths.
type FetchedAttachment = { name: string; content_type: string; buf: ArrayBuffer };

async function fetchAttachments(attachments: Attachment[]): Promise<FetchedAttachment[]> {
    return Promise.all(attachments.map(async att => {
        const res = await fetch(att.url);
        if (!res.ok) throw new Error(`Failed to load attachment: ${att.name} (${res.status})`);
        return { name: att.name, content_type: att.content_type, buf: await res.arrayBuffer() };
    }));
}

function attachmentToBlocks(att: FetchedAttachment): any[] {
    const data = Buffer.from(att.buf).toString("base64");
    if (att.content_type.startsWith("image/"))
        return [{ type: "image_url", image_url: { url: `data:${att.content_type};base64,${data}` } }];
    if (att.content_type === "application/pdf")
        return [{ type: "file", mimeType: "application/pdf", data, metadata: { filename: att.name } }];
    // CSV, TXT, JSON, XML... → decode and inline as text
    return [{ type: "text", text: `\n\n--- ${att.name} ---\n${Buffer.from(att.buf).toString("utf-8")}\n---` }];
}

function attachmentFallbackBlock(att: FetchedAttachment): any {
    const isTextLike = att.content_type.startsWith("text/") ||
        ["application/json", "application/xml"].includes(att.content_type);
    if (isTextLike)
        return { type: "text", text: `\n\n--- ${att.name} ---\n${Buffer.from(att.buf).toString("utf-8")}\n---` };
    return { type: "text", text: `[Attached: ${att.name} (${att.content_type}) — not supported by this provider]` };
}

function isUnsupportedContentError(e: unknown): boolean {
    const msg = (e as any)?.message ?? "";
    return msg.includes("Unsupported content block") || msg.includes("not supported");
}

async function streamAgent(messages: any[], invokeId: string, onChunk?: (t: string) => void): Promise<string> {
    const stream = await agent.stream(
        { messages },
        { streamMode: "messages" as const, recursionLimit: 150, configurable: { invokeId } },
    );
    let response = "";
    for await (const [chunk, metadata] of stream) {
        if (metadata.langgraph_node !== "model_request") continue;
        const text = chunk.text ?? "";
        if (text) { response += text; onChunk?.(text); }
    }
    return response;
}

async function runAgent(message: string, history: any[], attachments: Attachment[], invokeId: string, onChunk?: (text: string) => void): Promise<string> {
    if (attachments.length === 0)
        return streamAgent([...history, { role: "user", content: message }], invokeId, onChunk);

    // Fetch all bytes once — nonce URLs are single-use, must not fetch twice.
    const fetched = await fetchAttachments(attachments);
    const userMessage = { role: "user", content: [{ type: "text", text: message }, ...fetched.flatMap(attachmentToBlocks)] };

    try {
        return await streamAgent([...history, userMessage], invokeId, onChunk);
    } catch (e) {
        if (isUnsupportedContentError(e)) {
            const fallback = { role: "user", content: [{ type: "text", text: message }, ...fetched.map(attachmentFallbackBlock)] };
            return await streamAgent([...history, fallback], invokeId, onChunk);
        }
        throw e;
    }
}

// --- Invoke handlers ---

async function invoke(m: any) {
    const attachments: Attachment[] = m.attachments ?? [];
    if (!agent || !m.invoke_id || (!m.message && attachments.length === 0)) {
        write({ type: "agent_error", invoke_id: m.invoke_id ?? "", error: "agent not ready or missing fields" });
        return;
    }
    try {
        const response = await runAgent(
            m.message ?? "",
            m.history ?? [],
            attachments,
            m.invoke_id,
            (delta) => write({ type: "agent_chunk", invoke_id: m.invoke_id, delta }),
        );
        write({ type: "agent_done", invoke_id: m.invoke_id, response });
    } catch (e: any) {
        write({ type: "agent_error", invoke_id: m.invoke_id, error: e.message ?? String(e) });
    }
}

async function invokeJob(m: any) {
    if (!agent || !m.id) {
        write({ type: "job_result", id: m.id ?? "", error: "agent not ready" });
        return;
    }
    try {
        const message = m.payload?.message ?? JSON.stringify(m.payload ?? {});
        await runAgent(message, [], [], crypto.randomUUID());
        write({ type: "job_result", id: m.id });
    } catch (e: any) {
        write({ type: "job_result", id: m.id, error: e.message ?? String(e) });
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
