import { BaseCheckpointSaver, WRITES_IDX_MAP, compareWritesSortKeys, writesSortKey,
  type Checkpoint, type CheckpointMetadata, type CheckpointTuple, type CheckpointListOptions, type PendingWrite,
} from "@langchain/langgraph-checkpoint";
import type { RunnableConfig } from "@langchain/core/runnables";
import { CoreState } from "./state";
const encode = (s: string) => Buffer.from(s).toString("base64url");
type Saved = { checkpoint: Checkpoint; metadata: CheckpointMetadata; parent?: string };
type Write = { task: string; path: string; index: number; channel: string; value: unknown };

export class CoreCheckpointer extends BaseCheckpointSaver {
  constructor(readonly state: CoreState, readonly thread: string) { super(); }
  private ns(config: RunnableConfig) {
    if (config.configurable?.thread_id !== this.thread) throw new Error("Checkpoint thread mismatch");
    return encode(config.configurable?.checkpoint_ns ?? "");
  }
  private config(ns: string, id: string): RunnableConfig {
    return { configurable: { thread_id: this.thread, checkpoint_ns: Buffer.from(ns, "base64url").toString(), checkpoint_id: id } };
  }
  async getTuple(config: RunnableConfig): Promise<CheckpointTuple | undefined> {
    const ns = this.ns(config);
    const id = config.configurable?.checkpoint_id ?? (await this.state.keys(`c/${ns}/`)).at(-1)?.split("/").at(-1);
    if (!id) return;
    const saved = await this.state.get<Saved>(`c/${ns}/${id}`);
    if (!saved) return;
    const writes = await Promise.all((await this.state.keys(`w/${ns}/${id}/`)).map(key => this.state.get<Write>(key)));
    const ordered = writes.filter((v): v is Write => !!v).sort((a, b) =>
      compareWritesSortKeys(writesSortKey(a.path, a.task, a.index), writesSortKey(b.path, b.task, b.index)));
    return { config: this.config(ns, id), checkpoint: saved.checkpoint, metadata: saved.metadata,
      parentConfig: saved.parent ? this.config(ns, saved.parent) : undefined,
      pendingWrites: ordered.map(w => [w.task, w.channel, w.value]),
    };
  }
  async *list(config: RunnableConfig, options: CheckpointListOptions = {}): AsyncGenerator<CheckpointTuple> {
    const ns = this.ns(config);
    let count = 0;
    for (const key of (await this.state.keys(`c/${ns}/`)).reverse()) {
      const id = key.split("/").at(-1)!;
      if (options.before?.configurable?.checkpoint_id && id >= options.before.configurable.checkpoint_id) continue;
      const tuple = await this.getTuple(this.config(ns, id));
      if (!tuple || Object.entries(options.filter ?? {}).some(([k, v]) => (tuple.metadata as Record<string, unknown> | undefined)?.[k] !== v)) continue;
      if (count++ >= (options.limit ?? Infinity)) return;
      yield tuple;
    }
  }
  async put(config: RunnableConfig, checkpoint: Checkpoint, metadata: CheckpointMetadata): Promise<RunnableConfig> {
    const ns = this.ns(config);
    await this.state.put(`c/${ns}/${checkpoint.id}`, { checkpoint, metadata, parent: config.configurable?.checkpoint_id });
    return this.config(ns, checkpoint.id);
  }
  async putWrites(config: RunnableConfig, writes: PendingWrite[], task: string, path = "") {
    const ns = this.ns(config), id = config.configurable?.checkpoint_id;
    if (!id) throw new Error("Missing checkpoint ID");
    for (const [i, [channel, value]] of writes.entries()) {
      const index = WRITES_IDX_MAP[channel] ?? i;
      const key = `w/${ns}/${id}/${encode(task)}/${index}`;
      if (index >= 0 && await this.state.get(key)) continue;
      await this.state.put(key, { task, path, index, channel, value });
    }
  }
  async deleteThread(thread: string) {
    if (thread !== this.thread) throw new Error("Checkpoint thread mismatch");
    for (const prefix of ["c/", "w/"]) for (const key of await this.state.keys(prefix)) await this.state.remove(key);
  }
}
