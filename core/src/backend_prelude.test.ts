import { describe, test, expect, beforeAll, afterAll } from "bun:test";
import { spawn, type ChildProcess } from "node:child_process";
import { join } from "node:path";
import { writeFileSync, unlinkSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";

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
  tmpFiles.push(file);

  const proc = spawn("bun", ["--preload", PRELUDE, file], {
    stdio: ["pipe", "pipe", "pipe"],
  });

  let buffer = "";
  const pending: string[] = [];
  const waiters: Array<(line: string) => void> = [];

  proc.stdout!.setEncoding("utf-8");
  proc.stdout!.on("data", (chunk: string) => {
    buffer += chunk;
    let nl: number;
    while ((nl = buffer.indexOf("\n")) !== -1) {
      const line = buffer.slice(0, nl).trim();
      buffer = buffer.slice(nl + 1);
      if (!line) continue;
      const w = waiters.shift();
      if (w) w(line);
      else pending.push(line);
    }
  });

  return {
    send(msg) {
      proc.stdin!.write(JSON.stringify(msg) + "\n");
    },
    readLine(timeoutMs = 3000) {
      return new Promise((resolve, reject) => {
        const queued = pending.shift();
        if (queued) return resolve(JSON.parse(queued));
        const timer = setTimeout(() => {
          const idx = waiters.indexOf(handler);
          if (idx !== -1) waiters.splice(idx, 1);
          reject(new Error(`readLine: no output after ${timeoutMs}ms`));
        }, timeoutMs);
        const handler = (line: string) => {
          clearTimeout(timer);
          resolve(JSON.parse(line));
        };
        waiters.push(handler);
      });
    },
    async noOutput(waitMs = 300) {
      await new Promise((r) => setTimeout(r, waitMs));
      return pending.length === 0;
    },
    close() {
      return new Promise((resolve) => {
        proc.stdin!.end();
        const timer = setTimeout(() => { proc.kill(); resolve(); }, 3000);
        proc.on("exit", () => { clearTimeout(timer); resolve(); });
      });
    },
  };
}

// ─── Setup / teardown ────────────────────────────────────────────────────────

let tmpDir: string;
let seq = 0;
const tmpFiles: string[] = [];

beforeAll(() => {
  tmpDir = mkdtempSync(join(tmpdir(), "prelude-test-"));
});

afterAll(() => {
  for (const f of tmpFiles) try { unlinkSync(f); } catch {}
});

// ─── v2 protocol: serve()-based workers ──────────────────────────────────────

describe("v2: serve()", () => {
  test("discover responds with protocol version and methods", async () => {
    const w = spawnWorker(`serve({ rpc: { ping: () => "pong", echo: (p: any) => p } });`);
    w.send(DISCOVER);
    const msg = await w.readLine();
    expect(msg.type).toBe("discover");
    expect(msg.protocol).toBe(2);
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

    w.send({ type: "rpc", id: "r1", method: "createItem", params: { name: "test" } });

    // Prelude should emit collection_op before rpc_response
    const cop = await w.readLine();
    expect(cop.type).toBe("collection_op");
    expect(cop.op).toBe("insert");
    expect(cop.entity).toBe("items");
    expect(cop.data).toEqual({ name: "test" });

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
      try { lines.push(await w.readLine(1000)); } catch { break; }
    }
    const errLog = lines.find((m) => m.type === "log" && m.level === "error");
    expect(errLog?.message).toContain("serve() called twice");
    await w.close();
  });
});

// ─── v2 compat: old serve(handlers) flat signature ──────────────────────────

describe("v2 compat: old flat serve() signature", () => {
  test("serve({ method: fn }) works without rpc wrapper", async () => {
    const w = spawnWorker(`serve({ ping: () => "pong", echo: (p: any) => p });`);
    w.send(DISCOVER);

    // Skip the deprecation warning log
    let disc = await w.readLine();
    while (disc.type === "log") disc = await w.readLine();

    expect(disc.protocol).toBe(2);
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
