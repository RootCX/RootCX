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
