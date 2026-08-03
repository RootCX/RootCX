import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { RuntimeClient } from "./client";
import { sendStorageChunk } from "./storage-upload";

type FakeEventListener = (event: { loaded: number }) => void;

class FakeEventTarget {
  private listeners = new Map<string, FakeEventListener[]>();

  addEventListener(type: string, listener: FakeEventListener): void {
    const listeners = this.listeners.get(type) ?? [];
    listeners.push(listener);
    this.listeners.set(type, listeners);
  }

  dispatch(type: string, loaded = 0): void {
    for (const listener of this.listeners.get(type) ?? []) listener({ loaded });
  }
}

class FakeXMLHttpRequest extends FakeEventTarget {
  static instances: FakeXMLHttpRequest[] = [];

  readonly upload = new FakeEventTarget();
  readonly headers = new Map<string, string>();
  status = 0;
  responseText = "";
  aborted = false;
  method = "";

  constructor() {
    super();
    FakeXMLHttpRequest.instances.push(this);
  }

  open(method: string, _url: string): void {
    this.method = method;
  }

  setRequestHeader(name: string, value: string): void {
    this.headers.set(name, value);
  }

  send(_body: Blob): void {}

  abort(): void {
    this.aborted = true;
    this.dispatch("abort");
  }

  progress(uploadedBytes: number): void {
    this.upload.dispatch("progress", uploadedBytes);
  }

  respond(status: number, body: unknown): void {
    this.status = status;
    this.responseText = typeof body === "string" ? body : JSON.stringify(body);
    this.dispatch("load");
  }
}

function jsonResponse(body: unknown, status = 200) {
  return {
    ok: status >= 200 && status < 300,
    status,
    json: async () => body,
    text: async () => JSON.stringify(body),
  };
}

function uploadSession() {
  return {
    id: "upload-1",
    app_id: "catalog",
    name: "catalog.xlsx",
    content_type: "application/octet-stream",
    expected_size: 4,
    uploaded_size: 0,
    completed_file_id: null,
    created_at: "2026-08-03T10:00:00Z",
    expires_at: "2026-08-04T10:00:00Z",
    state: "uploading",
    max_chunk_size: 4,
  };
}

function storedFile() {
  return {
    file_id: "file-1",
    name: "catalog.xlsx",
    content_type: "application/octet-stream",
    size: 4,
    checksum: "abc",
  };
}

async function waitForRequests(count: number): Promise<void> {
  await vi.waitFor(() => expect(FakeXMLHttpRequest.instances).toHaveLength(count));
}

describe("browser storage chunk transport", () => {
  beforeEach(() => {
    FakeXMLHttpRequest.instances = [];
    vi.stubGlobal("XMLHttpRequest", FakeXMLHttpRequest);
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("reports transmitted bytes before the server response", async () => {
    const progress: number[] = [];
    const result = sendStorageChunk({
      url: "https://core.test/upload",
      token: "token",
      offset: 8,
      chunk: new Blob(["abcd"]),
      idleTimeoutMs: 1_000,
      onProgress: (uploadedBytes) => progress.push(uploadedBytes),
    });
    const request = FakeXMLHttpRequest.instances[0];

    request.progress(2);

    expect(progress).toEqual([2]);
    expect(request.method).toBe("PATCH");
    expect(request.headers.get("Upload-Offset")).toBe("8");
    expect(request.headers.get("Authorization")).toBe("Bearer token");

    request.respond(200, { uploaded_size: 12 });
    await expect(result).resolves.toEqual({
      status: 200,
      body: JSON.stringify({ uploaded_size: 12 }),
    });
  });

  it("aborts a chunk when no bytes move before the idle deadline", async () => {
    vi.useFakeTimers();
    const result = sendStorageChunk({
      url: "https://core.test/upload",
      token: null,
      offset: 0,
      chunk: new Blob(["abcd"]),
      idleTimeoutMs: 1_000,
    });
    const rejection = expect(result).rejects.toThrow("storage upload stalled for 1000 ms");

    await vi.advanceTimersByTimeAsync(1_000);

    expect(FakeXMLHttpRequest.instances[0].aborted).toBe(true);
    await rejection;
  });

  it("aborts the active request when the caller cancels the upload", async () => {
    const controller = new AbortController();
    const reason = new Error("upload cancelled");
    const result = sendStorageChunk({
      url: "https://core.test/upload",
      token: null,
      offset: 0,
      chunk: new Blob(["abcd"]),
      idleTimeoutMs: 1_000,
      signal: controller.signal,
    });

    controller.abort(reason);

    expect(FakeXMLHttpRequest.instances[0].aborted).toBe(true);
    await expect(result).rejects.toBe(reason);
  });

  it("extends the idle deadline only when additional bytes move", async () => {
    vi.useFakeTimers();
    const result = sendStorageChunk({
      url: "https://core.test/upload",
      token: null,
      offset: 0,
      chunk: new Blob(["abcd"]),
      idleTimeoutMs: 1_000,
    });
    const request = FakeXMLHttpRequest.instances[0];

    await vi.advanceTimersByTimeAsync(900);
    request.progress(2);
    await vi.advanceTimersByTimeAsync(900);

    expect(request.aborted).toBe(false);
    request.respond(200, { uploaded_size: 4 });
    await expect(result).resolves.toMatchObject({ status: 200 });
  });

  it("distinguishes transmitted progress from the durable offset", async () => {
    vi.stubGlobal("fetch", async (url: string, init?: RequestInit) => {
      if (url.endsWith("/storage/uploads") && init?.method === "POST") {
        return jsonResponse(uploadSession(), 201);
      }
      if (url.endsWith("/complete") && init?.method === "POST") {
        return jsonResponse(storedFile(), 201);
      }
      throw new Error(`unexpected request: ${init?.method ?? "GET"} ${url}`);
    });
    const client = new RuntimeClient({ baseUrl: "https://core.test" });
    client.setTokens("token", null);
    const progress: Array<[number, number | undefined]> = [];
    const upload = client.uploadFileResumable("catalog", new Blob(["abcd"]), {
      name: "catalog.xlsx",
      onProgress: ({ uploadedBytes, transmittedBytes }) => {
        progress.push([uploadedBytes, transmittedBytes]);
      },
    });
    await waitForRequests(1);

    FakeXMLHttpRequest.instances[0].progress(2);
    expect(progress).toEqual([[0, 0], [0, 2]]);

    FakeXMLHttpRequest.instances[0].respond(200, { uploaded_size: 4 });
    await upload;
    expect(progress.at(-1)).toEqual([4, 4]);
  });

  it("refreshes an expired token before retrying the same chunk", async () => {
    vi.stubGlobal("fetch", async (url: string, init?: RequestInit) => {
      if (url.endsWith("/storage/uploads") && init?.method === "POST") {
        return jsonResponse(uploadSession(), 201);
      }
      if (url.endsWith("/auth/refresh") && init?.method === "POST") {
        return jsonResponse({ accessToken: "fresh-token" });
      }
      if (url.endsWith("/complete") && init?.method === "POST") {
        return jsonResponse(storedFile(), 201);
      }
      throw new Error(`unexpected request: ${init?.method ?? "GET"} ${url}`);
    });
    const client = new RuntimeClient({ baseUrl: "https://core.test" });
    client.setTokens("expired-token", "refresh-token");
    const upload = client.uploadFileResumable("catalog", new Blob(["abcd"]), {
      name: "catalog.xlsx",
    });
    await waitForRequests(1);

    FakeXMLHttpRequest.instances[0].respond(401, "expired");
    await waitForRequests(2);

    expect(FakeXMLHttpRequest.instances[1].headers.get("Authorization")).toBe("Bearer fresh-token");
    expect(FakeXMLHttpRequest.instances[1].headers.get("Upload-Offset")).toBe("0");
    FakeXMLHttpRequest.instances[1].respond(200, { uploaded_size: 4 });
    await upload;
  });
});
