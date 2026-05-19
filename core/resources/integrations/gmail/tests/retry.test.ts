import { describe, it, expect, beforeAll } from "bun:test";
import { withRetry, ok, fail, _setSleep, type Result } from "../lib/errors";

beforeAll(() => {
  _setSleep(async () => {});
});

describe("withRetry", () => {
  it("TEMPORARY_ERROR retries up to 4 calls total, returns last error", async () => {
    let calls = 0;
    const fn = async (): Promise<Result<string>> => {
      calls++;
      return fail({ code: "TEMPORARY_ERROR", message: `fail ${calls}` });
    };
    const r = await withRetry(fn);
    expect(r.ok).toBe(false);
    expect(calls).toBe(4);
    if (!r.ok) expect(r.error.message).toBe("fail 4");
  });

  it("TEMPORARY_ERROR succeeding on 2nd attempt -> 2 calls, returns ok", async () => {
    let calls = 0;
    const fn = async (): Promise<Result<string>> => {
      calls++;
      if (calls === 1) return fail({ code: "TEMPORARY_ERROR", message: "transient" });
      return ok("done");
    };
    const r = await withRetry(fn);
    expect(r.ok).toBe(true);
    expect(calls).toBe(2);
    if (r.ok) expect(r.data).toBe("done");
  });

  const noRetryCodes = ["INSUFFICIENT_PERMISSIONS", "SYNC_CURSOR_ERROR", "NOT_FOUND", "MISCONFIGURED", "UNKNOWN"] as const;
  for (const code of noRetryCodes) {
    it(`${code} -> 1 call only, no retry`, async () => {
      let calls = 0;
      const fn = async (): Promise<Result<string>> => {
        calls++;
        return fail({ code, message: "no retry" });
      };
      const r = await withRetry(fn);
      expect(r.ok).toBe(false);
      expect(calls).toBe(1);
    });
  }

  it("success on first call -> 1 call, returns ok", async () => {
    let calls = 0;
    const fn = async (): Promise<Result<number>> => {
      calls++;
      return ok(42);
    };
    const r = await withRetry(fn);
    expect(r.ok).toBe(true);
    expect(calls).toBe(1);
    if (r.ok) expect(r.data).toBe(42);
  });
});
