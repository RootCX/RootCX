import { IpcReader, IpcWriter } from "./ipc.js";
import { runAgent, type AgentConfig } from "./runner.js";

const reader = new IpcReader(process.stdin);
const writer = new IpcWriter(process.stdout);

reader.on("discover", () => {
    writer.send({ type: "discover", capabilities: ["agent"] });
});

reader.on("agent_invoke", async (msg) => {
    try {
        await runAgent({
            sessionId: msg.session_id as string,
            message: msg.message as string,
            systemPrompt: msg.system_prompt as string,
            config: msg.config as AgentConfig,
            history: (msg.history as Array<Record<string, unknown>>) ?? [],
            writer,
        });
    } catch (err) {
        writer.send({
            type: "agent_error",
            session_id: msg.session_id as string,
            error: err instanceof Error ? err.message : String(err),
        });
    }
});

writer.send({ type: "log", level: "info", message: "agent runtime ready" });
