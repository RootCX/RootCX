import { describe, it, expect } from "bun:test";
import { classifyHttp, parseRetryAfter } from "../lib/errors";

describe("classifyHttp", () => {
  const cases: Array<[number, string, string | undefined, string, boolean?]> = [
    // [status, body, path, expectedCode, hasRetryAfter?]
    [400, '{"error":{"errors":[{"reason":"invalid_grant","message":"bad token"}]}}', undefined, "INSUFFICIENT_PERMISSIONS"],
    [400, '{"error":{"errors":[{"reason":"failedPrecondition","message":"Mail service not enabled"}]}}', undefined, "INSUFFICIENT_PERMISSIONS"],
    [400, '{"error":{"errors":[{"reason":"failedPrecondition","message":"something else"}]}}', undefined, "TEMPORARY_ERROR"],
    [400, '{"error":{"errors":[{"reason":"badRequest","message":"invalid param"}]}}', undefined, "UNKNOWN"],
    [401, '{"error":"unauthorized"}', undefined, "INSUFFICIENT_PERMISSIONS"],
    [403, '{"error":{"errors":[{"reason":"rateLimitExceeded","message":"rate limit. Retry after 2030-01-01T00:00:00Z"}]}}', undefined, "TEMPORARY_ERROR", true],
    [403, '{"error":{"errors":[{"reason":"userRateLimitExceeded","message":"too fast"}]}}', undefined, "TEMPORARY_ERROR"],
    [403, '{"error":{"errors":[{"reason":"dailyLimitExceeded","message":"daily quota"}]}}', undefined, "TEMPORARY_ERROR"],
    [403, '{"error":{"errors":[{"reason":"domainPolicy","message":"blocked"}]}}', undefined, "INSUFFICIENT_PERMISSIONS"],
    [403, '{"error":{"errors":[{"reason":"insufficientPermissions","message":"no scope"}]}}', undefined, "INSUFFICIENT_PERMISSIONS"],
    [404, '{"error":{"errors":[{"reason":"notFound","message":"historyId not found"}]}}', "/history?startHistoryId=123", "SYNC_CURSOR_ERROR"],
    [404, '{"error":{"errors":[{"reason":"notFound","message":"msg not found"}]}}', "/messages/abc123", "NOT_FOUND"],
    [429, '{"error":{"errors":[{"reason":"rateLimitExceeded","message":"Retry after 2030-06-01T12:00:00Z"}]}}', undefined, "TEMPORARY_ERROR", true],
    [500, '{"error":{"errors":[{"reason":"backendError","message":"backend down"}]}}', undefined, "TEMPORARY_ERROR"],
    [502, '{"error":{"errors":[{"reason":"internal_failure","message":"internal"}]}}', undefined, "TEMPORARY_ERROR"],
    [503, '{"error":"unavailable"}', undefined, "TEMPORARY_ERROR"],
    [504, '{"error":{"errors":[{"reason":"backendError","message":"timeout"}]}}', undefined, "TEMPORARY_ERROR"],
    [504, '{"error":{"message":"Authentication backend unavailable"}}', undefined, "TEMPORARY_ERROR"],
    [418, 'teapot', undefined, "UNKNOWN"],
  ];

  for (const [status, body, path, expectedCode, hasRetry] of cases) {
    const reasonLabel = (() => { try { return JSON.parse(body)?.error?.errors?.[0]?.reason ?? "?"; } catch { return "?"; } })();
    it(`${status} ${path ?? ""} reason=${reasonLabel} -> ${expectedCode}`, () => {
      const err = classifyHttp(status, body, path);
      expect(err.code).toBe(expectedCode);
      if (hasRetry) expect(err.retryAfter).toBeGreaterThan(Date.now());
    });
  }
});

describe("parseRetryAfter", () => {
  const cases: Array<[string, boolean]> = [
    ["Retry after 2030-01-01T00:00:00Z", true],
    ["Retry after 2030-01-01T00:00:00.123Z", true],
    ["Retry after 2020-01-01T00:00:00Z", false],
    ["no retry hint here", false],
    ["Retry after garbage", false],
    ["", false],
  ];

  for (const [msg, expected] of cases) {
    it(`"${msg.slice(0, 40)}" -> ${expected ? "Date" : "undefined"}`, () => {
      const result = parseRetryAfter(msg);
      if (expected) {
        expect(result).toBeDefined();
        expect(result!).toBeGreaterThan(Date.now());
      } else {
        expect(result).toBeUndefined();
      }
    });
  }
});
