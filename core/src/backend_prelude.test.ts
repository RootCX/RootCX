import { afterAll, afterEach, beforeAll, describe, expect, test } from "bun:test";
import { spawn } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

// ─── Helpers ─────────────────────────────────────────────────────────────────

const PRELUDE = join(import.meta.dir, "backend_prelude.js");

const DISCOVER = {
  type: "discover",
  app_id: "test-app",
  runtime_url: "http://localhost:9100",
  credentials: {},
  agent_config: null,
  // Lifecycle worker: onStart runs only when the core sets this.
  run_onstart: true,
};

interface Worker {
  send(msg: Record<string, unknown>): void;
  readLine(timeoutMs?: number): Promise<any>;
  noOutput(waitMs?: number): Promise<boolean>;
  close(): Promise<void>;
}

function spawnWorker(script: string): Worker {
  const file = join(tmpDir, `worker-${++seq}.ts`);
  writeFileSync(file, script);

  const proc = spawn("bun", ["--preload", PRELUDE, file], {
    stdio: ["pipe", "pipe", "pipe"],
  });

  let buffer = "";
  const pending: string[] = [];
  const waiters: Array<{ line(line: string): void; reject(error: Error): void }> = [];
  let stopped: Error | undefined;
  let closing: Promise<void> | undefined;
  // Subscribe at spawn time: a short-lived worker may exit before close().
  // "close" also guarantees stdout/stderr have drained after process exit.
  const exited = new Promise<void>((resolve) => {
    proc.once("error", (error) => { stopped = error; });
    proc.once("close", (code, signal) => {
      stopped ??= new Error(`worker exited (code=${code}, signal=${signal})`);
      for (const waiter of waiters.splice(0)) waiter.reject(stopped);
      resolve();
    });
  });
  proc.stderr!.resume();
  proc.stdin!.on("error", (error) => { stopped ??= error; });

  proc.stdout!.setEncoding("utf-8");
  proc.stdout!.on("data", (chunk: string) => {
    buffer += chunk;
    let nl: number;
    while ((nl = buffer.indexOf("\n")) !== -1) {
      const line = buffer.slice(0, nl).trim();
      buffer = buffer.slice(nl + 1);
      if (!line) continue;
      const w = waiters.shift();
      if (w) w.line(line);
      else pending.push(line);
    }
  });

  async function stopWorker(): Promise<void> {
    // Requests can be intentionally unresolved; EOF would leave their timers running.
    proc.stdin!.destroy();
    const timer = setTimeout(() => { proc.kill("SIGKILL"); }, 3000);
    try {
      if (proc.exitCode === null && proc.signalCode === null) proc.kill();
      await exited;
    } finally {
      clearTimeout(timer);
      workers.delete(worker);
    }
  }

  const worker: Worker = {
    send(msg) {
      proc.stdin!.write(JSON.stringify(msg) + "\n");
    },
    readLine(timeoutMs = 3000) {
      return new Promise((resolve, reject) => {
        const queued = pending.shift();
        if (queued) return resolve(JSON.parse(queued));
        if (stopped) return reject(stopped);
        const timer = setTimeout(() => {
          const idx = waiters.indexOf(handler);
          if (idx !== -1) waiters.splice(idx, 1);
          reject(new Error(`readLine: no output after ${timeoutMs}ms`));
        }, timeoutMs);
        const handler = {
          line(line: string) {
            clearTimeout(timer);
            try { resolve(JSON.parse(line)); } catch (error) { reject(error); }
          },
          reject(error: Error) {
            clearTimeout(timer);
            reject(error);
          },
        };
        waiters.push(handler);
      });
    },
    async noOutput(waitMs = 300) {
      await new Promise((r) => setTimeout(r, waitMs));
      return pending.length === 0;
    },
    close() {
      return closing ??= stopWorker();
    },
  };
  workers.add(worker);
  return worker;
}

function spawnCollectionWorker(): Worker {
  return spawnWorker(`
    serve({ rpc: {
      async call(params: any, _caller: any, ctx: any) {
        const collection = params.remote
          ? ctx.remote("catalog").collection("items")
          : ctx.collection("items");
        return collection[params.method](...params.args);
      },
    } });
  `);
}

// ─── Setup / teardown ────────────────────────────────────────────────────────

let tmpDir: string;
let seq = 0;
const workers = new Set<Worker>();

beforeAll(() => {
  tmpDir = mkdtempSync(join(tmpdir(), "prelude-test-"));
});

afterEach(async () => {
  await Promise.all([...workers].map((worker) => worker.close()));
});

afterAll(() => {
  rmSync(tmpDir, { recursive: true, force: true });
});

// ─── v4 protocol: serve()-based workers ──────────────────────────────────────

describe("v4: serve()", () => {
  test("discover responds with protocol version and methods", async () => {
    const w = spawnWorker(`serve({ rpc: { ping: () => "pong", echo: (p: any) => p } });`);
    w.send(DISCOVER);
    const msg = await w.readLine();
    expect(msg.type).toBe("discover");
    expect(msg.protocol).toBe(5);
    expect(msg.methods).toEqual(["ping", "echo"]);
    await w.close();
  });

  test("rpc dispatches to handler and returns result", async () => {
    const w = spawnWorker(`serve({ rpc: { add: (p: any) => p.a + p.b } });`);
    w.send(DISCOVER);
    await w.readLine(); // consume discover response
    w.send({ type: "rpc", id: "r1", method: "add", params: { a: 2, b: 3 } });
    const msg = await w.readLine();
    expect(msg).toEqual({ type: "rpc_response", id: "r1", result: 5 });
    await w.close();
  });

  test("rpc with unknown method returns error", async () => {
    const w = spawnWorker(`serve({ rpc: { ping: () => "pong" } });`);
    w.send(DISCOVER);
    await w.readLine();
    w.send({ type: "rpc", id: "r2", method: "nope", params: {} });
    const msg = await w.readLine();
    expect(msg.type).toBe("rpc_response");
    expect(msg.id).toBe("r2");
    expect(msg.error).toContain("unknown method");
    await w.close();
  });

  test("rpc handler error is returned as error string", async () => {
    const w = spawnWorker(`serve({ rpc: { fail: () => { throw new Error("boom"); } } });`);
    w.send(DISCOVER);
    await w.readLine();
    w.send({ type: "rpc", id: "r3", method: "fail", params: {} });
    const msg = await w.readLine();
    expect(msg).toEqual({ type: "rpc_response", id: "r3", error: "boom" });
    await w.close();
  });

  test("job dispatches to onJob handler", async () => {
    const w = spawnWorker(`serve({ onJob: (p: any) => ({ sum: p.x + p.y }) });`);
    w.send(DISCOVER);
    await w.readLine();
    w.send({ type: "job", id: "j1", payload: { x: 10, y: 20 } });
    const msg = await w.readLine();
    expect(msg).toEqual({ type: "job_result", id: "j1", result: { sum: 30 } });
    await w.close();
  });

  test("job without onJob returns default ok", async () => {
    const w = spawnWorker(`serve({ rpc: { ping: () => "pong" } });`);
    w.send(DISCOVER);
    await w.readLine();
    w.send({ type: "job", id: "j2", payload: {} });
    const msg = await w.readLine();
    expect(msg).toEqual({ type: "job_result", id: "j2", result: { ok: true } });
    await w.close();
  });

  test("onStart fires once even on repeated discover", async () => {
    const w = spawnWorker(`
      let count = 0;
      serve({
        onStart() { count++; log.info("start:" + count); },
        rpc: { getCount: () => count },
      });
    `);
    w.send(DISCOVER);
    await w.readLine(); // discover response
    await w.readLine(); // log "start:1"
    // Second discover
    w.send(DISCOVER);
    await w.readLine(); // second discover response
    // Query the count
    w.send({ type: "rpc", id: "r1", method: "getCount", params: {} });
    const msg = await w.readLine();
    expect(msg.result).toBe(1);
    await w.close();
  });

  test("onStart does NOT fire for a non-lifecycle worker (run_onstart absent)", async () => {
    // Per-user/agent workers receive discover WITHOUT run_onstart. onStart must
    // not run for them: it bypasses RLS for self-schema and must never execute
    // under a user identity. Inverting/dropping the gate would regress here.
    const w = spawnWorker(`
      let count = 0;
      serve({
        onStart() { count++; },
        rpc: { getCount: () => count },
      });
    `);
    const { run_onstart, ...userDiscover } = DISCOVER;
    w.send(userDiscover);
    await w.readLine(); // discover response (always sent)
    w.send({ type: "rpc", id: "r1", method: "getCount", params: {} });
    const msg = await w.readLine();
    expect(msg.result).toBe(0);
    await w.close();
  });

  test("ctx.collection round-trip: insert emits collection_op, resolves on result", async () => {
    const w = spawnWorker(`
      serve({
        rpc: {
          async createItem(params: any, _caller: any, ctx: any) {
            return ctx.collection("items").insert(params);
          },
        },
      });
    `);
    w.send(DISCOVER);
    await w.readLine(); // discover

    w.send({
      type: "rpc", id: "r1", invocation_id: "core-invocation-1",
      method: "createItem", params: { name: "test" },
    });

    // Prelude should emit collection_op before rpc_response
    const cop = await w.readLine();
    expect(cop.type).toBe("collection_op");
    expect(cop.op).toBe("insert");
    expect(cop.entity).toBe("items");
    expect(cop.data).toEqual({ name: "test" });
    expect(cop.invocation_id).toBe("core-invocation-1");

    // Simulate Core sending back the result
    w.send({ type: "collection_op_result", id: cop.id, result: { id: "1", name: "test" } });

    const rpc = await w.readLine();
    expect(rpc).toEqual({
      type: "rpc_response",
      id: "r1",
      result: { id: "1", name: "test" },
    });
    await w.close();
  });

  test("collections separate full equality arrays from explicit pages on both transports", async () => {
    const rows = Array.from({ length: 105 }, (_, id) => ({ id: String(id) }));
    const reserved = { limit: 7, offset: 3, order: "customer-order", orderBy: "label", where: "tag" };
    const options = { where: { limit: 7 }, orderBy: "name", order: "ASC", limit: 2, offset: 100 };
    for (const remote of [false, true]) {
      const w = spawnCollectionWorker();
      w.send(DISCOVER);
      await w.readLine();
      for (const [method, args, data, result] of [
        ["find", [], {}, rows],
        ["find", [reserved], reserved, [rows[0]]],
        ["findPage", [], {}, { data: rows.slice(0, 100), total: 105 }],
        ["findPage", [options], options, { data: rows.slice(100, 102), total: 105 }],
        ["findOne", [reserved], reserved, rows[0]],
        ["findOne", [{ id: "missing" }], { id: "missing" }, null],
        ["findOne", [], {}, rows[0]],
      ] as const) {
        const label = `${remote ? "remote" : "local"}.${method}(${JSON.stringify(args)})`;
        w.send({
          type: "rpc", id: label, invocation_id: "core-read",
          method: "call", params: { remote, method, args },
        });
        const op = await w.readLine();
        expect(op, label).toEqual({
          type: remote ? "remote_collection_op" : "collection_op",
          ...(remote ? { provider_app: "catalog" } : {}),
          id: expect.any(String), invocation_id: "core-read", entity: "items",
          op: remote && method === "find" ? "findAll" : method,
          data,
        });
        w.send({ type: "collection_op_result", id: op.id, result });
        expect(await w.readLine(), label).toEqual({ type: "rpc_response", id: label, result });
      }
      await w.close();
    }
  });

  test("collections preserve create aliases and both update signatures on both transports", async () => {
    for (const remote of [false, true]) {
      const w = spawnCollectionWorker();
      w.send(DISCOVER);
      await w.readLine();
      for (const [method, args, localOp, remoteOp, localData, remoteData] of [
        ["create", [{ name: "new" }], "insert", "create", { name: "new" }, { name: "new" }],
        ["insert", [{ name: "alias" }], "insert", "create", { name: "alias" }, { name: "alias" }],
        ["update", [{ id: "record-id", name: "flat" }], "update", "update",
          { id: "record-id", name: "flat" }, { id: "record-id", data: { name: "flat" } }],
        ["update", ["record-id", { name: "separate" }], "update", "update",
          { id: "record-id", name: "separate" }, { id: "record-id", data: { name: "separate" } }],
        ["delete", ["record-id"], "delete", "delete", { id: "record-id" }, { id: "record-id" }],
      ] as const) {
        const label = `${remote ? "remote" : "local"}.${method}(${JSON.stringify(args)})`;
        w.send({
          type: "rpc", id: label, invocation_id: "core-mutation",
          method: "call", params: { remote, method, args },
        });
        const op = await w.readLine();
        expect(op, label).toEqual({
          type: remote ? "remote_collection_op" : "collection_op",
          ...(remote ? { provider_app: "catalog" } : {}),
          id: expect.any(String), invocation_id: "core-mutation", entity: "items",
          op: remote ? remoteOp : localOp,
          data: remote ? remoteData : localData,
        });
        const result = method === "delete" ? { id: "record-id", deleted: true } : { id: "record-id" };
        w.send({ type: "collection_op_result", id: op.id, result });
        expect(await w.readLine(), label).toEqual({ type: "rpc_response", id: label, result });
      }
      await w.close();
    }
  });

  test("collections propagate Core denials and invalid-update errors instead of successful empty results", async () => {
    for (const remote of [false, true]) {
      const w = spawnCollectionWorker();
      w.send(DISCOVER);
      await w.readLine();
      for (const [method, args, error] of [
        ["findOne", [{ id: "record-id" }], "read denied"],
        ["create", [{ secret: "hidden" }], "field is not writable"],
        ["update", [{ id: "record-id" }], "no fields to update"],
        ["delete", ["missing"], "record not found"],
      ] as const) {
        const label = `${remote ? "remote" : "local"}.${method}(${JSON.stringify(args)})`;
        w.send({ type: "rpc", id: label, method: "call", params: { remote, method, args } });
        const op = await w.readLine();
        expect(op.type, label).toBe(remote ? "remote_collection_op" : "collection_op");
        w.send({ type: "collection_op_result", id: op.id, error });
        expect(await w.readLine(), label).toEqual({ type: "rpc_response", id: label, error });
      }
      await w.close();
    }
  });

  test("ctx.transaction serializes statements, commits, and returns the callback value", async () => {
    const w = spawnWorker(`
      serve({ rpc: {
        async save(_params: any, _caller: any, ctx: any) {
          return ctx.transaction(async (tx: any) => {
            const first = tx.sql("INSERT first RETURNING id", [1]);
            const second = tx.sql("INSERT second RETURNING id", [2]);
            const [a, b] = await Promise.all([first, second]);
            return { ids: [a.rows[0][0], b.rows[0][0]] };
          });
        },
      } });
    `);
    w.send(DISCOVER);
    await w.readLine();
    w.send({
      type: "rpc", id: "r1", invocation_id: "core-invocation-tx",
      method: "save", params: {},
    });

    const begin = await w.readLine();
    expect(begin.type).toBe("sql_begin");
    expect(begin.invocation_id).toBe("core-invocation-tx");
    w.send({ type: "sql_begin_result", id: begin.id, tx_id: "core-tx" });

    const first = await w.readLine();
    expect(first).toMatchObject({ type: "sql_exec", tx_id: "core-tx", sql: "INSERT first RETURNING id", params: [1] });
    expect(first.invocation_id).toBe("core-invocation-tx");
    expect(await w.noOutput()).toBe(true);
    w.send({ type: "sql_exec_result", id: first.id, rows: [["a"]], columns: ["id"], row_count: 1 });

    const second = await w.readLine();
    expect(second).toMatchObject({ type: "sql_exec", tx_id: "core-tx", sql: "INSERT second RETURNING id", params: [2] });
    w.send({ type: "sql_exec_result", id: second.id, rows: [["b"]], columns: ["id"], row_count: 1 });

    const commit = await w.readLine();
    expect(commit).toMatchObject({ type: "sql_commit", tx_id: "core-tx" });
    expect(commit.invocation_id).toBe("core-invocation-tx");
    w.send({ type: "sql_end_result", id: commit.id });
    expect(await w.readLine()).toEqual({ type: "rpc_response", id: "r1", result: { ids: ["a", "b"] } });
    await w.close();
  });

  test("a tx.sql error poisons the callback outcome even when application code catches it", async () => {
    const w = spawnWorker(`
      serve({ rpc: {
        async save(_p: any, _c: any, ctx: any) {
          return ctx.transaction(async (tx: any) => {
            try { await tx.sql("BROKEN"); } catch (_) {}
            return "must-not-commit";
          });
        },
      } });
    `);
    w.send(DISCOVER);
    await w.readLine();
    w.send({ type: "rpc", id: "r1", method: "save", params: {} });
    const begin = await w.readLine();
    w.send({ type: "sql_begin_result", id: begin.id, tx_id: "core-tx" });
    const exec = await w.readLine();
    w.send({ type: "sql_exec_result", id: exec.id, error: "constraint failed" });
    const rollback = await w.readLine();
    expect(rollback.type).toBe("sql_rollback");
    w.send({ type: "sql_end_result", id: rollback.id });
    expect(await w.readLine()).toEqual({ type: "rpc_response", id: "r1", error: "constraint failed" });
    await w.close();
  });

  test("callback failure rolls back and keeps the original error if rollback also fails", async () => {
    const w = spawnWorker(`
      serve({ rpc: {
        fail: (_p: any, _c: any, ctx: any) => ctx.transaction(() => { throw new Error("business failure"); }),
      } });
    `);
    w.send(DISCOVER);
    await w.readLine();
    w.send({ type: "rpc", id: "r1", method: "fail", params: {} });
    const begin = await w.readLine();
    w.send({ type: "sql_begin_result", id: begin.id, tx_id: "core-tx" });
    const rollback = await w.readLine();
    expect(rollback.type).toBe("sql_rollback");
    w.send({ type: "sql_end_result", id: rollback.id, error: "connection lost" });
    const logLine = await w.readLine();
    expect(logLine).toMatchObject({ type: "log", level: "error" });
    expect(logLine.message).toContain("connection lost");
    expect(await w.readLine()).toEqual({ type: "rpc_response", id: "r1", error: "business failure" });
    await w.close();
  });

  test("transaction rejects nesting", async () => {
    const w = spawnWorker(`
      serve({ rpc: {
        async inspect(_p: any, _c: any, ctx: any) {
          let error = "";
          await ctx.transaction(async (tx: any) => {
            try { await ctx.transaction(() => null); } catch (e: any) { error = e.message; }
          });
          return error;
        },
      } });
    `);
    w.send(DISCOVER);
    await w.readLine();
    w.send({ type: "rpc", id: "r1", method: "inspect", params: {} });
    const begin = await w.readLine();
    w.send({ type: "sql_begin_result", id: begin.id, tx_id: "core-tx" });
    const commit = await w.readLine();
    expect(commit.type).toBe("sql_commit");
    w.send({ type: "sql_end_result", id: commit.id });
    const response = await w.readLine();
    expect(response.result).toBe("nested transactions are not supported; use the current tx");
    await w.close();
  });

  test("transaction exposes only tx.sql while its callback is active", async () => {
    const collectionCalls = [
      ["find", []], ["findPage", []], ["findOne", []],
      ["create", [{ name: "new" }]], ["insert", [{ name: "alias" }]],
      ["update", [{ id: "record-id", name: "flat" }]],
      ["update", ["record-id", { name: "separate" }]], ["delete", ["record-id"]],
    ];
    const w = spawnWorker(`
      serve({ rpc: {
        async inspect(_p: any, _c: any, ctx: any) {
          const errors: Array<{ capability: string; error: string }> = [];
          const calls: Array<[string, () => any]> = [];
          for (const [name, collection] of [
            ["ctx.collection", ctx.collection("items")],
            ["ctx.remote", ctx.remote("catalog").collection("items")],
          ] as const) {
            for (const [method, args] of ${JSON.stringify(collectionCalls)}) {
              calls.push([name + "." + method, () => collection[method](...args)]);
            }
          }
          await ctx.transaction(async (_tx: any) => {
            for (const [capability, call] of [
              ["ctx.sql", () => ctx.sql("SELECT outside")],
              ...calls,
              ["global emit", () => globalThis.emit("outside-effect")],
              ["global uploadFile", () => globalThis.uploadFile("x", "x.txt", "text/plain")],
            ]) {
              try { await call(); }
              catch (e: any) { errors.push({ capability, error: e.message }); }
            }
          });
          return errors;
        },
      } });
    `);
    w.send(DISCOVER);
    await w.readLine();
    w.send({ type: "rpc", id: "r1", method: "inspect", params: {} });
    const begin = await w.readLine();
    w.send({ type: "sql_begin_result", id: begin.id, tx_id: "core-tx" });
    const commit = await w.readLine();
    expect(commit.type).toBe("sql_commit");
    w.send({ type: "sql_end_result", id: commit.id });
    const response = await w.readLine();
    expect(response.result).toEqual([
      { capability: "ctx.sql", error: expect.stringContaining("ctx.sql cannot be used inside") },
      ...["ctx.collection", "ctx.remote"].flatMap((name) => collectionCalls.map(([method]) => ({
        capability: `${name}.${method}`, error: expect.stringContaining(`${name} cannot be used inside`),
      }))),
      { capability: "global emit", error: expect.stringContaining("emit cannot be used inside") },
      { capability: "global uploadFile", error: expect.stringContaining("uploadFile cannot be used inside") },
    ]);
    await w.close();
  });

  test("transaction capability guard is isolated to its async invocation", async () => {
    const w = spawnWorker(`
      let release: () => void;
      serve({ rpc: {
        hold: (_p: any, _c: any, ctx: any) => ctx.transaction(
          () => new Promise<void>((resolve) => {
            release = resolve;
            log.info("transaction callback is waiting");
          }),
        ),
        outside: () => {
          globalThis.emit("outside-allowed");
          release();
          return "released";
        },
      } });
    `);
    w.send(DISCOVER);
    await w.readLine();
    w.send({ type: "rpc", id: "holding", method: "hold", params: {} });
    const begin = await w.readLine();
    w.send({ type: "sql_begin_result", id: begin.id, tx_id: "core-tx" });
    expect(await w.readLine()).toMatchObject({ type: "log", level: "info" });

    w.send({ type: "rpc", id: "outside", method: "outside", params: {} });
    const event = await w.readLine();
    expect(event).toMatchObject({ type: "event", name: "outside-allowed" });

    const messages = [await w.readLine(), await w.readLine()];
    const outsideResponse = messages.find((message) => message.type === "rpc_response");
    const commit = messages.find((message) => message.type === "sql_commit");
    expect(outsideResponse).toEqual({ type: "rpc_response", id: "outside", result: "released" });
    expect(commit).toMatchObject({ type: "sql_commit", tx_id: "core-tx" });
    w.send({ type: "sql_end_result", id: commit.id });
    expect(await w.readLine()).toEqual({ type: "rpc_response", id: "holding" });
    await w.close();
  });

  test("transaction handle expires when its callback returns", async () => {
    const w = spawnWorker(`
      let leaked: any;
      serve({ rpc: {
        async inspect(_p: any, _c: any, ctx: any) {
          await ctx.transaction(async (tx: any) => { leaked = tx; });
          try { await leaked.sql("SELECT late"); }
          catch (e: any) { return e.message; }
        },
      } });
    `);
    w.send(DISCOVER);
    await w.readLine();
    w.send({ type: "rpc", id: "r1", method: "inspect", params: {} });
    const begin = await w.readLine();
    w.send({ type: "sql_begin_result", id: begin.id, tx_id: "core-tx" });
    const commit = await w.readLine();
    expect(commit.type).toBe("sql_commit");
    w.send({ type: "sql_end_result", id: commit.id });
    const response = await w.readLine();
    expect(response.result).toBe("transaction is no longer active");
    await w.close();
  });

  test("ctx.uploadFile emits storage_upload", async () => {
    const w = spawnWorker(`
      serve({
        rpc: {
          async upload(_p: any, _c: any, ctx: any) {
            return ctx.uploadFile("hello", "test.txt", "text/plain");
          },
        },
      });
    `);
    w.send(DISCOVER);
    await w.readLine(); // discover

    w.send({ type: "rpc", id: "r1", method: "upload", params: {} });

    // Skip log messages (uploadFile logs "[storage] upload start")
    let msg = await w.readLine();
    while (msg.type === "log") msg = await w.readLine();

    expect(msg.type).toBe("storage_upload");
    expect(msg.name).toBe("test.txt");
    expect(msg.content_type).toBe("text/plain");
    expect(msg.size).toBe(5); // "hello" = 5 bytes
    await w.close();
  });

  test("ctx.downloadFile emits storage_download and resolves content", async () => {
    const w = spawnWorker(`
      serve({
        rpc: {
          async download(_p: any, _c: any, ctx: any) {
            const file = await ctx.downloadFile("peppol", "11111111-1111-1111-1111-111111111111");
            return {
              appId: file.appId,
              fileId: file.fileId,
              name: file.name,
              contentType: file.contentType,
              text: new TextDecoder().decode(file.content),
            };
          },
        },
      });
    `);
    w.send(DISCOVER);
    await w.readLine(); // discover

    w.send({ type: "rpc", id: "r1", method: "download", params: {} });
    const req = await w.readLine();
    expect(req).toEqual({
      type: "storage_download",
      id: expect.any(String),
      app_id: "peppol",
      file_id: "11111111-1111-1111-1111-111111111111",
    });

    w.send({
      type: "storage_download_result",
      id: req.id,
      app_id: "peppol",
      file_id: "11111111-1111-1111-1111-111111111111",
      name: "invoice.xml",
      content_type: "application/xml",
      size: 5,
      url: "data:application/octet-stream;base64,aGVsbG8=",
    });

    const msg = await w.readLine();
    expect(msg).toEqual({
      type: "rpc_response",
      id: "r1",
      result: {
        appId: "peppol",
        fileId: "11111111-1111-1111-1111-111111111111",
        name: "invoice.xml",
        contentType: "application/xml",
        text: "hello",
      },
    });
    await w.close();
  });

  test("ctx.openFile never uses the buffered download path", async () => {
    const w = spawnWorker(`
      Response.prototype.arrayBuffer = () => { throw new Error("buffered download used"); };
      serve({
        rpc: {
          async download(_p: any, _c: any, ctx: any) {
            const file = await ctx.openFile("kova_erp", "11111111-1111-1111-1111-111111111111");
            return {
              size: file.size,
              text: await new Response(file.stream).text(),
            };
          },
        },
      });
    `);
    w.send(DISCOVER);
    await w.readLine();
    w.send({ type: "rpc", id: "r1", method: "download", params: {} });
    const req = await w.readLine();
    expect(req.type).toBe("storage_download");

    w.send({
      type: "storage_download_result",
      id: req.id,
      app_id: "kova_erp",
      file_id: "11111111-1111-1111-1111-111111111111",
      name: "catalog.xlsx",
      content_type: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
      size: 5,
      url: "data:application/octet-stream;base64,aGVsbG8=",
    });

    expect(await w.readLine()).toEqual({
      type: "rpc_response",
      id: "r1",
      result: { size: 5, text: "hello" },
    });
    await w.close();
  });

  test("ctx.enqueueJob emits job_enqueue and resolves msg id", async () => {
    const w = spawnWorker(`
      serve({
        rpc: {
          async queue(_p: any, _c: any, ctx: any) {
            return ctx.enqueueJob({ type: "export", export_id: "exp1" });
          },
        },
      });
    `);
    w.send(DISCOVER);
    await w.readLine(); // discover

    w.send({ type: "rpc", id: "r1", method: "queue", params: {} });
    const req = await w.readLine();
    expect(req).toEqual({
      type: "job_enqueue",
      id: expect.any(String),
      payload: { type: "export", export_id: "exp1" },
    });

    w.send({ type: "job_enqueue_result", id: req.id, msg_id: 42 });
    const msg = await w.readLine();
    expect(msg).toEqual({
      type: "rpc_response",
      id: "r1",
      result: { msgId: 42 },
    });
    await w.close();
  });

  test("jobs execute serially within one worker", async () => {
    const w = spawnWorker(`
      let active = 0;
      serve({
        onJob: async (payload: any) => {
          active++;
          log.info(\`start:\${payload.n}:active:\${active}\`);
          await new Promise((resolve) => setTimeout(resolve, 50));
          active--;
          return { n: payload.n };
        },
      });
    `);
    w.send(DISCOVER);
    await w.readLine();

    w.send({ type: "job", id: "1", payload: { n: 1 } });
    w.send({ type: "job", id: "2", payload: { n: 2 } });

    const messages = [];
    while (messages.filter((m) => m.type === "job_result").length < 2) {
      messages.push(await w.readLine(1000));
    }
    const starts = messages
      .filter((m) => m.type === "log" && m.message.startsWith("start:"))
      .map((m) => m.message);
    expect(starts).toEqual(["start:1:active:1", "start:2:active:1"]);
    await w.close();
  });

  test("serve() called twice throws", async () => {
    const w = spawnWorker(`
      serve({ rpc: {} });
      try { serve({ rpc: {} }); } catch (e: any) { log.error(e.message); }
    `);
    w.send(DISCOVER);

    // Read lines until we find the error log
    const lines = [];
    for (let i = 0; i < 5; i++) {
      const msg = await w.readLine(1000);
      lines.push(msg);
      if (msg.type === "log" && msg.level === "error") break;
    }
    const errLog = lines.find((m) => m.type === "log" && m.level === "error");
    expect(errLog?.message).toContain("serve() called twice");
    await w.close();
  });
});

// ─── v4 compat: old serve(handlers) flat signature ──────────────────────────

describe("v4 compat: old flat serve() signature", () => {
  test("serve({ method: fn }) works without rpc wrapper", async () => {
    const w = spawnWorker(`serve({ ping: () => "pong", echo: (p: any) => p });`);
    w.send(DISCOVER);

    // Skip the deprecation warning log
    let disc = await w.readLine();
    while (disc.type === "log") disc = await w.readLine();

    expect(disc.protocol).toBe(5);
    expect(disc.methods).toEqual(["ping", "echo"]);

    w.send({ type: "rpc", id: "r1", method: "ping", params: {} });
    const msg = await w.readLine();
    expect(msg).toEqual({ type: "rpc_response", id: "r1", result: "pong" });
    await w.close();
  });
});

// ─── v1 protocol: legacy workers (no serve()) ───────────────────────────────

describe("v1: legacy (no serve())", () => {
  const LEGACY_SCRIPT = `
    // Legacy app: own stdin handler, no serve()
    process.stdin.setEncoding("utf-8");
    let buf = "";
    process.stdin.on("data", (chunk: string) => {
      buf += chunk;
      let nl: number;
      while ((nl = buf.indexOf("\\n")) !== -1) {
        const line = buf.slice(0, nl).trim();
        buf = buf.slice(nl + 1);
        if (!line) continue;
        const msg = JSON.parse(line);
        if (msg.type === "discover") {
          process.stdout.write(JSON.stringify({ type: "discover", methods: ["legacy"] }) + "\\n");
        }
        if (msg.type === "rpc") {
          process.stdout.write(JSON.stringify({ type: "rpc_response", id: msg.id, result: "legacy" }) + "\\n");
        }
      }
    });
  `;

  test("prelude does NOT respond to discover — only the legacy app does", async () => {
    const w = spawnWorker(LEGACY_SCRIPT);
    w.send(DISCOVER);
    const msg = await w.readLine();
    // Legacy app response — no "protocol" field, has "legacy" method
    expect(msg.type).toBe("discover");
    expect(msg.protocol).toBeUndefined();
    expect(msg.methods).toEqual(["legacy"]);
    // Verify no second discover response from prelude
    expect(await w.noOutput()).toBe(true);
    await w.close();
  });

  test("prelude does NOT respond to rpc — only the legacy app does", async () => {
    const w = spawnWorker(LEGACY_SCRIPT);
    w.send(DISCOVER);
    await w.readLine(); // legacy discover
    w.send({ type: "rpc", id: "r1", method: "legacy", params: {} });
    const msg = await w.readLine();
    expect(msg).toEqual({ type: "rpc_response", id: "r1", result: "legacy" });
    expect(await w.noOutput()).toBe(true);
    await w.close();
  });

  test("globalThis.uploadFile works without serve()", async () => {
    const w = spawnWorker(`
      // Legacy: no serve(), but uses globalThis.uploadFile
      process.stdin.setEncoding("utf-8");
      let buf = "";
      process.stdin.on("data", (chunk: string) => {
        buf += chunk;
        let nl: number;
        while ((nl = buf.indexOf("\\n")) !== -1) {
          const line = buf.slice(0, nl).trim();
          buf = buf.slice(nl + 1);
          if (!line) continue;
          const msg = JSON.parse(line);
          if (msg.type === "discover") {
            // After discover, try uploadFile
            (globalThis as any).uploadFile("data", "file.txt", "text/plain")
              .catch(() => {}); // ignore timeout, we just test it emits
          }
        }
      });
    `);
    w.send(DISCOVER);

    // Read lines until we find storage_upload (skip logs)
    let found = false;
    for (let i = 0; i < 5; i++) {
      try {
        const msg = await w.readLine(1000);
        if (msg.type === "storage_upload") {
          expect(msg.name).toBe("file.txt");
          found = true;
          break;
        }
      } catch { break; }
    }
    expect(found).toBe(true);
    await w.close();
  });

  test("globalThis.downloadFile works without serve()", async () => {
    const w = spawnWorker(`
      process.stdin.setEncoding("utf-8");
      let buf = "";
      process.stdin.on("data", (chunk: string) => {
        buf += chunk;
        let nl: number;
        while ((nl = buf.indexOf("\\n")) !== -1) {
          const line = buf.slice(0, nl).trim();
          buf = buf.slice(nl + 1);
          if (!line) continue;
          const msg = JSON.parse(line);
          if (msg.type === "discover") {
            (globalThis as any).downloadFile("peppol", "11111111-1111-1111-1111-111111111111")
              .catch(() => {});
          }
        }
      });
    `);
    w.send(DISCOVER);

    let found = false;
    for (let i = 0; i < 5; i++) {
      try {
        const msg = await w.readLine(1000);
        if (msg.type === "storage_download") {
          expect(msg.app_id).toBe("peppol");
          expect(msg.file_id).toBe("11111111-1111-1111-1111-111111111111");
          found = true;
          break;
        }
      } catch { break; }
    }
    expect(found).toBe(true);
    await w.close();
  });

  test("globalThis.log works without serve()", async () => {
    const w = spawnWorker(`log.info("hello from legacy");`);
    const msg = await w.readLine();
    expect(msg).toEqual({ type: "log", level: "info", message: "hello from legacy" });
    await w.close();
  });
});
