import { describe, it, expect, vi, beforeEach } from "vitest";
import { RuntimeClient } from "./client";

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
