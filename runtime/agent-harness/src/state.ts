import { createHash } from "node:crypto";
import { MemorySaver } from "@langchain/langgraph-checkpoint";

export type StateRequest = (op: string, namespace: string, key: string, value?: unknown) => Promise<any>;
const serde = new MemorySaver().serde;

/** Chunked content-addressed values keep binary documents below the IPC frame limit. */
export class CoreState {
  constructor(readonly request: StateRequest, readonly namespace: string) {}
  async get<T>(key: string): Promise<T | undefined> {
    const ref = await this.request("get", this.namespace, key);
    if (!ref) return;
    const chunks: string[] = [];
    for (let i = 0; i < ref.chunks; i++) {
      const part = await this.request("get", this.namespace, `blob/${ref.hash}/${i}`);
      if (typeof part !== "string") throw new Error("Incomplete saved agent state");
      chunks.push(part);
    }
    return await serde.loadsTyped(ref.type, Buffer.from(chunks.join(""), "base64")) as T;
  }
  async put(key: string, value: unknown) {
    const [type, bytes] = await serde.dumpsTyped(value);
    const hash = createHash("sha256").update(bytes).digest("hex");
    const data = Buffer.from(bytes).toString("base64");
    const chunks = Math.max(1, Math.ceil(data.length / 450_000));
    for (let i = 0; i < chunks; i++) {
      const id = `blob/${hash}/${i}`;
      if (await this.request("get", this.namespace, id) == null)
        await this.request("put", this.namespace, id, data.slice(i * 450_000, (i + 1) * 450_000));
    }
    await this.request("put", this.namespace, key, { type, hash, chunks });
  }
  async remove(key: string) { await this.request("delete", this.namespace, key); }
  async keys(prefix: string): Promise<string[]> {
    const keys: string[] = [];
    let after = "";
    for (;;) {
      const page: string[] = await this.request("list", this.namespace, prefix, after);
      keys.push(...page);
      if (page.length < 500) return keys;
      after = page.at(-1)!;
    }
  }
}
