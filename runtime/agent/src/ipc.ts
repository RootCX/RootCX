import { createInterface } from "readline";

export type MessageHandler = (msg: Record<string, unknown>) => void;

export class IpcReader {
    private handlers = new Map<string, MessageHandler>();

    constructor(input: NodeJS.ReadableStream) {
        const rl = createInterface({ input, crlfDelay: Infinity });
        rl.on("line", (line: string) => {
            if (!line.trim()) return;
            try {
                const msg = JSON.parse(line);
                const handler = this.handlers.get(msg.type);
                if (handler) handler(msg);
            } catch (e) {
                process.stderr.write(`ipc: failed to parse line: ${e}\n`);
            }
        });
    }

    on(type: string, handler: MessageHandler) {
        this.handlers.set(type, handler);
    }
}

export class IpcWriter {
    constructor(private output: NodeJS.WritableStream) {}

    send(msg: Record<string, unknown>) {
        this.output.write(JSON.stringify(msg) + "\n");
    }
}
