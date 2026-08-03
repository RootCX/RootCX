import { describe, it, expect, vi, beforeEach } from "vitest";
import { RuntimeClient, AgentEvent } from "./client";

function storageSession(uploadedSize: number, maxChunkSize = 4) {
  return {
    id: "upload-1",
    app_id: "catalog",
    name: "catalog.xlsx",
    content_type: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    expected_size: 10,
    uploaded_size: uploadedSize,
    completed_file_id: null,
    created_at: "2026-08-03T10:00:00Z",
    expires_at: "2026-08-04T10:00:00Z",
    state: "uploading",
    max_chunk_size: maxChunkSize,
  };
}

function jsonResponse(body: unknown, status = 200) {
  return {
    ok: status >= 200 && status < 300,
    status,
    json: async () => body,
    text: async () => JSON.stringify(body),
  };
}

describe("resumable storage upload", () => {
  let client: RuntimeClient;

  beforeEach(() => {
    client = new RuntimeClient({ baseUrl: "http://localhost:9100" });
    client.setTokens("tok", null);
  });

  it("obeys the server chunk limit and reports committed progress", async () => {
    let uploadedSize = 0;
    const patchOffsets: number[] = [];
    vi.stubGlobal("fetch", async (url: string, init?: RequestInit) => {
      const method = init?.method ?? "GET";
      if (url.endsWith("/storage/uploads") && method === "POST") {
        return jsonResponse(storageSession(0), 201);
      }
      if (url.endsWith("/storage/uploads/upload-1") && method === "PATCH") {
        const offset = Number(new Headers(init?.headers).get("Upload-Offset"));
        const chunk = init?.body as Blob;
        patchOffsets.push(offset);
        uploadedSize += chunk.size;
        return jsonResponse({ upload_id: "upload-1", uploaded_size: uploadedSize });
      }
      if (url.endsWith("/storage/uploads/upload-1/complete") && method === "POST") {
        return jsonResponse({
          file_id: "file-1",
          name: "catalog.xlsx",
          content_type: "application/octet-stream",
          size: 10,
          checksum: "abc",
        }, 201);
      }
      throw new Error(`unexpected request: ${method} ${url}`);
    });

    const progress: number[] = [];
    await client.uploadFileResumable(
      "catalog",
      new Blob(["0123456789"]),
      {
        name: "catalog.xlsx",
        onProgress: ({ uploadedBytes }) => progress.push(uploadedBytes),
      },
    );

    expect(patchOffsets).toEqual([0, 4, 8]);
    expect(progress).toEqual([0, 4, 8, 10]);
  });

  it("recovers the committed offset after an ambiguous network failure", async () => {
    let uploadedSize = 0;
    let firstPatch = true;
    const patchOffsets: number[] = [];
    vi.stubGlobal("fetch", async (url: string, init?: RequestInit) => {
      const method = init?.method ?? "GET";
      if (url.endsWith("/storage/uploads") && method === "POST") {
        return jsonResponse(storageSession(0, 5), 201);
      }
      if (url.endsWith("/storage/uploads/upload-1") && method === "PATCH") {
        const offset = Number(new Headers(init?.headers).get("Upload-Offset"));
        patchOffsets.push(offset);
        uploadedSize = offset + (init?.body as Blob).size;
        if (firstPatch) {
          firstPatch = false;
          throw new TypeError("connection reset after commit");
        }
        return jsonResponse({ upload_id: "upload-1", uploaded_size: uploadedSize });
      }
      if (url.endsWith("/storage/uploads/upload-1") && method === "GET") {
        return jsonResponse(storageSession(uploadedSize, 5));
      }
      if (url.endsWith("/storage/uploads/upload-1/complete") && method === "POST") {
        return jsonResponse({
          file_id: "file-1",
          name: "catalog.xlsx",
          content_type: "application/octet-stream",
          size: 10,
          checksum: "abc",
        });
      }
      throw new Error(`unexpected request: ${method} ${url}`);
    });

    await client.uploadFileResumable("catalog", new Blob(["0123456789"]), {
      name: "catalog.xlsx",
      maxRetries: 1,
    });

    expect(patchOffsets).toEqual([0, 5]);
  });

  it("resumes an existing session from its durable offset", async () => {
    vi.stubGlobal("fetch", async (url: string, init?: RequestInit) => {
      const method = init?.method ?? "GET";
      if (url.endsWith("/storage/uploads/upload-1") && method === "GET") {
        return jsonResponse(storageSession(5, 5));
      }
      if (url.endsWith("/storage/uploads/upload-1") && method === "PATCH") {
        expect(new Headers(init?.headers).get("Upload-Offset")).toBe("5");
        return jsonResponse({ upload_id: "upload-1", uploaded_size: 10 });
      }
      if (url.endsWith("/storage/uploads/upload-1/complete") && method === "POST") {
        return jsonResponse({
          file_id: "file-1",
          name: "catalog.xlsx",
          content_type: "application/octet-stream",
          size: 10,
          checksum: "abc",
        });
      }
      throw new Error(`unexpected request: ${method} ${url}`);
    });

    await client.uploadFileResumable("catalog", new Blob(["0123456789"]), {
      name: "catalog.xlsx",
      uploadId: "upload-1",
    });
  });

  it("retries creation with the same upload id after an ambiguous failure", async () => {
    const createdIds: string[] = [];
    vi.stubGlobal("fetch", async (url: string, init?: RequestInit) => {
      const method = init?.method ?? "GET";
      if (url.endsWith("/storage/uploads") && method === "POST") {
        const body = JSON.parse(init?.body as string) as { upload_id: string };
        createdIds.push(body.upload_id);
        if (createdIds.length === 1) {
          throw new TypeError("connection reset after create commit");
        }
        return jsonResponse({
          ...storageSession(0),
          id: body.upload_id,
          expected_size: 1,
        }, 200);
      }
      const uploadId = createdIds[0];
      if (url.endsWith(`/storage/uploads/${uploadId}`) && method === "PATCH") {
        return jsonResponse({ upload_id: uploadId, uploaded_size: 1 });
      }
      if (url.endsWith(`/storage/uploads/${uploadId}/complete`) && method === "POST") {
        return jsonResponse({
          file_id: "file-1",
          name: "catalog.xlsx",
          content_type: "application/octet-stream",
          size: 1,
          checksum: "abc",
        }, 201);
      }
      throw new Error(`unexpected request: ${method} ${url}`);
    });

    await client.uploadFileResumable("catalog", new Blob(["x"]), {
      name: "catalog.xlsx",
      maxRetries: 1,
    });

    expect(createdIds).toHaveLength(2);
    expect(createdIds[0]).toBe(createdIds[1]);
  });

  it("retries completion after an ambiguous network failure", async () => {
    let completionAttempts = 0;
    vi.stubGlobal("fetch", async (url: string, init?: RequestInit) => {
      const method = init?.method ?? "GET";
      if (url.endsWith("/storage/uploads") && method === "POST") {
        return jsonResponse(storageSession(0), 201);
      }
      if (url.endsWith("/storage/uploads/upload-1") && method === "PATCH") {
        return jsonResponse({ upload_id: "upload-1", uploaded_size: 10 });
      }
      if (url.endsWith("/storage/uploads/upload-1/complete") && method === "POST") {
        completionAttempts += 1;
        if (completionAttempts === 1) {
          throw new TypeError("connection reset after completion commit");
        }
        return jsonResponse({
          file_id: "file-1",
          name: "catalog.xlsx",
          content_type: "application/octet-stream",
          size: 10,
          checksum: "abc",
        });
      }
      throw new Error(`unexpected request: ${method} ${url}`);
    });

    await client.uploadFileResumable("catalog", new Blob(["0123456789"]), {
      name: "catalog.xlsx",
      maxRetries: 1,
    });

    expect(completionAttempts).toBe(2);
  });
});

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
