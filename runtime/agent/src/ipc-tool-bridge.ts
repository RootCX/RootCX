import type { IpcWriter } from "./ipc.js";

interface PendingCall {
    resolve: (value: unknown) => void;
    reject: (error: Error) => void;
}

export class IpcToolBridge {
    private pending = new Map<string, PendingCall>();

    constructor(private writer: IpcWriter) {}

    handleResult(msg: { call_id: string; result?: unknown; error?: string }) {
        const p = this.pending.get(msg.call_id);
        if (p) {
            this.pending.delete(msg.call_id);
            if (msg.error) {
                p.reject(new Error(msg.error));
            } else {
                p.resolve(msg.result ?? null);
            }
        }
    }

    async callTool(invokeId: string, toolName: string, args: unknown): Promise<unknown> {
        const callId = crypto.randomUUID();
        return new Promise<unknown>((resolve, reject) => {
            this.pending.set(callId, { resolve, reject });
            this.writer.send({
                type: "agent_tool_call",
                invoke_id: invokeId,
                call_id: callId,
                tool_name: toolName,
                args,
            });
        });
    }
}
