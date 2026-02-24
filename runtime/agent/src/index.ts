import { IpcReader, IpcWriter } from "./ipc.js";
import { runAgent, type AgentConfig } from "./runner.js";

function assertInvoke(msg: Record<string, unknown>): asserts msg is {
    invoke_id: string; message: string; system_prompt: string;
    config: AgentConfig; auth_token: string; history?: unknown[];
} {
    if (typeof msg.invoke_id !== "string") throw new Error("missing invoke_id");
    if (typeof msg.message !== "string") throw new Error("missing message");
    if (typeof msg.system_prompt !== "string") throw new Error("missing system_prompt");
    if (typeof msg.auth_token !== "string") throw new Error("missing auth_token");
    if (!msg.config || typeof msg.config !== "object") throw new Error("missing config");
}

const reader = new IpcReader(process.stdin);
const writer = new IpcWriter(process.stdout);

reader.on("discover", () => {
    writer.send({ type: "discover", capabilities: ["agent"] });
});

reader.on("agent_invoke", async (msg) => {
    assertInvoke(msg);
    try {
        await runAgent({
            invokeId: msg.invoke_id,
            message: msg.message,
            systemPrompt: msg.system_prompt,
            config: msg.config,
            authToken: msg.auth_token,
            history: (msg.history as Array<Record<string, unknown>>) ?? [],
            writer,
        });
    } catch (err) {
        writer.send({
            type: "agent_error",
            invoke_id: msg.invoke_id,
            error: err instanceof Error ? err.message : String(err),
        });
    }
});

writer.send({ type: "log", level: "info", message: "agent runtime ready" });
