import { BaseStore, InMemoryStore, type Operation, type OperationResults, type Item } from "@langchain/langgraph-checkpoint";
import { CoreState } from "./state";

// The library owns query semantics; Core owns durability and access boundaries.
export class CoreStore extends BaseStore {
  constructor(readonly state: CoreState) { super(); }
  private key(namespace: string[], key: string) { return `item/${Buffer.from(JSON.stringify([namespace, key])).toString("base64url")}`; }
  async batch<Op extends readonly Operation[]>(operations: Op): Promise<OperationResults<Op>> {
    const results: unknown[] = [];
    for (const op of operations) {
      if ("namespace" in op && "key" in op) {
        const key = this.key(op.namespace, op.key);
        const existing = await this.state.get<Item>(key);
        if (!("value" in op)) { results.push(existing ?? null); continue; }
        if (op.value === null) await this.state.remove(key);
        else await this.state.put(key, { namespace: op.namespace, key: op.key, value: op.value,
          createdAt: existing?.createdAt ?? new Date(), updatedAt: new Date() });
        results.push(undefined);
      } else {
        const memory = new InMemoryStore();
        const items = await Promise.all((await this.state.keys("item/")).map(key => this.state.get<Item>(key)));
        for (const item of items) if (item) await memory.put(item.namespace, item.key, item.value);
        results.push((await memory.batch([op]))[0]);
      }
    }
    return results as OperationResults<Op>;
  }
}
