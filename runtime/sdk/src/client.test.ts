import { describe, it, expect, vi, beforeEach } from "vitest";
import { RuntimeClient, AgentEvent } from "./client";

describe("client.core()", () => {
  let client: RuntimeClient;
  let fetchedUrls: string[];

  beforeEach(() => {
    client = new RuntimeClient({ baseUrl: "http://localhost:9100" });
    client.setTokens("tok", null);
    fetchedUrls = [];
    vi.stubGlobal("fetch", async (url: string) => {
      fetchedUrls.push(url);
      return { ok: true, status: 200, json: async () => [] };
    });
  });

  it("routes core collection calls to correct endpoints", async () => {
    const cases = [
      { call: () => client.core().collection("users").list(), expected: "/api/v1/users" },
      { call: () => client.core().collection("users").get("abc-123"), expected: "/api/v1/users/abc-123" },
    ];
    for (const { call, expected } of cases) {
      fetchedUrls = [];
      await call();
      expect(fetchedUrls[0]).toBe(`http://localhost:9100${expected}`);
    }
  });

  it("rejects unknown core entity", () => {
    expect(() => client.core().collection("unknown")).toThrow("unknown core entity");
  });
});

describe("cron methods", () => {
  let client: RuntimeClient;
  let calls: { url: string; method: string; body?: unknown }[];

  beforeEach(() => {
    client = new RuntimeClient({ baseUrl: "http://localhost:9100" });
    client.setTokens("tok", null);
    calls = [];
    vi.stubGlobal("fetch", async (url: string, init?: RequestInit) => {
      const body = init?.body ? JSON.parse(init.body as string) : undefined;
      calls.push({ url, method: init?.method ?? "GET", body });
      return { ok: true, status: 200, json: async () => ({ id: "c1" }) };
    });
  });

  it("routes cron CRUD to correct endpoints and methods", async () => {
    const cases = [
      { call: () => client.listCrons("app1"), url: "/apps/app1/crons", method: "GET" },
      { call: () => client.createCron("app1", { name: "x", schedule: "* * * * *" }), url: "/apps/app1/crons", method: "POST" },
      { call: () => client.updateCron("app1", "c1", { enabled: false }), url: "/apps/app1/crons/c1", method: "PATCH" },
      { call: () => client.deleteCron("app1", "c1"), url: "/apps/app1/crons/c1", method: "DELETE" },
      { call: () => client.triggerCron("app1", "c1"), url: "/apps/app1/crons/c1/trigger", method: "POST" },
    ];
    for (const { call, url, method } of cases) {
      calls = [];
      await call();
      expect(calls[0].url).toBe(`http://localhost:9100/api/v1${url}`);
      expect(calls[0].method).toBe(method);
    }
  });

  it("sends correct body for create and update", async () => {
    await client.createCron("a", { name: "test", schedule: "10 seconds", payload: { k: 1 } });
    expect(calls[0].body).toEqual({ name: "test", schedule: "10 seconds", payload: { k: 1 } });

    calls = [];
    await client.updateCron("a", "c1", { enabled: false, overlapPolicy: "queue" });
    expect(calls[0].body).toEqual({ enabled: false, overlapPolicy: "queue" });
  });

  it("throws RuntimeApiError on non-ok response", async () => {
    vi.stubGlobal("fetch", async () => ({
      ok: false, status: 400, text: async () => "bad schedule",
    }));
    await expect(client.createCron("a", { name: "x", schedule: "bad" })).rejects.toThrow("bad schedule");
  });
});

function sseStream(chunks: string[]) {
  const encoder = new TextEncoder();
  let i = 0;
  return {
    getReader: () => ({
      read: async () => {
        if (i >= chunks.length) return { done: true, value: undefined };
        return { done: false, value: encoder.encode(chunks[i++]) };
      },
      cancel: async () => {},
    }),
  };
}

function stubFetchSSE(chunks: string[]) {
  vi.stubGlobal("fetch", async () => ({
    ok: true,
    status: 200,
    body: sseStream(chunks),
  }));
}

describe("invokeAgent", () => {
  let client: RuntimeClient;

  beforeEach(() => {
    client = new RuntimeClient({ baseUrl: "http://localhost:9100" });
    client.setTokens("tok", null);
  });

  it("parses chunk and done events from SSE stream", async () => {
    stubFetchSSE([
      'event: chunk\ndata: {"delta":"hello","session_id":"s1"}\n\n',
      'event: done\ndata: {"response":"hello world","session_id":"s1","tokens":42}\n\n',
    ]);
    const events: AgentEvent[] = [];
    const result = await client.invokeAgent("agent1", { message: "hi" }, (e) => events.push(e));

    expect(events).toHaveLength(2);
    expect(events[0]).toEqual({ type: "chunk", delta: "hello", sessionId: "s1" });
    expect(result).toEqual({ type: "done", response: "hello world", sessionId: "s1", tokens: 42 });
  });

  it("ignores SSE comments and handles multi-line data", async () => {
    stubFetchSSE([
      ':keepalive\nevent: done\ndata: {"response":"ok",\ndata: "session_id":"s1","tokens":1}\n\n',
    ]);
    const events: AgentEvent[] = [];
    const result = await client.invokeAgent("agent1", { message: "x" }, (e) => events.push(e));

    expect(result.response).toBe("ok");
  });

  it("skips malformed JSON data lines without crashing", async () => {
    stubFetchSSE([
      'event: chunk\ndata: {broken json\n\nevent: done\ndata: {"response":"r","session_id":"s","tokens":0}\n\n',
    ]);
    const events: AgentEvent[] = [];
    const result = await client.invokeAgent("agent1", { message: "x" }, (e) => events.push(e));

    expect(events).toHaveLength(1);
    expect(result.type).toBe("done");
  });

  it("throws error event message when stream ends without done", async () => {
    stubFetchSSE([
      'event: error\ndata: {"error":"model overloaded","session_id":"s1"}\n\n',
    ]);
    const events: AgentEvent[] = [];
    await expect(
      client.invokeAgent("agent1", { message: "x" }, (e) => events.push(e)),
    ).rejects.toThrow("model overloaded");
    expect(events[0]).toEqual({ type: "error", error: "model overloaded", sessionId: "s1" });
  });

  it("throws generic message when stream ends with no events", async () => {
    stubFetchSSE([""]);
    await expect(
      client.invokeAgent("agent1", { message: "x" }, () => {}),
    ).rejects.toThrow("agent stream ended without done event");
  });

  it("handles events split across multiple chunks", async () => {
    stubFetchSSE([
      'event: chunk\n',
      'data: {"delta":"hi","session_id":"s1"}\n\n',
      'event: done\ndata: {"response":"hi","session_id":"s1","tokens":5}\n\n',
    ]);
    const events: AgentEvent[] = [];
    const result = await client.invokeAgent("agent1", { message: "x" }, (e) => events.push(e));

    expect(events).toHaveLength(2);
    expect(events[0]).toEqual({ type: "chunk", delta: "hi", sessionId: "s1" });
    expect(result.response).toBe("hi");
  });

  it("sends sessionId and fileIds when provided (including empty string)", async () => {
    let sentBody: Record<string, unknown> = {};
    vi.stubGlobal("fetch", async (_url: string, init?: RequestInit) => {
      sentBody = JSON.parse(init?.body as string);
      return {
        ok: true,
        status: 200,
        body: sseStream(['event: done\ndata: {"response":"","session_id":"","tokens":0}\n\n']),
      };
    });

    await client.invokeAgent(
      "agent1",
      { message: "hi", sessionId: "", fileIds: ["f1"] },
      () => {},
    );

    expect(sentBody.session_id).toBe("");
    expect(sentBody.file_ids).toEqual(["f1"]);
  });
});
