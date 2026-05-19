import { describe, it, expect } from "bun:test";
import { classifyHttp, parseRetryAfter } from "../lib/errors";

describe("classifyHttp", () => {
  const cases: Array<[number, string, string, boolean?]> = [
    [400, '{"error":{"errors":[{"reason":"invalid_grant","message":"bad token"}]}}', "INSUFFICIENT_PERMISSIONS"],
    [400, '{"error":{"errors":[{"reason":"timeRangeEmpty","message":"empty range"}]}}', "MISCONFIGURED"],
    [400, '{"error":{"errors":[{"reason":"required","message":"missing field"}]}}', "MISCONFIGURED"],
    [401, '{"error":"unauthorized"}', "INSUFFICIENT_PERMISSIONS"],
    [403, '{"error":{"errors":[{"reason":"rateLimitExceeded","message":"rate limit. Retry after 2030-01-01T00:00:00Z"}]}}', "TEMPORARY_ERROR", true],
    [403, '{"error":{"errors":[{"reason":"userRateLimitExceeded","message":"too fast"}]}}', "TEMPORARY_ERROR"],
    [403, '{"error":{"errors":[{"reason":"quotaExceeded","message":"daily limit"}]}}', "TEMPORARY_ERROR"],
    [403, '{"error":{"errors":[{"reason":"forbidden","message":"access denied"}]}}', "INSUFFICIENT_PERMISSIONS"],
    [403, '{"error":{"errors":[{"reason":"forbiddenForNonOrganizer","message":"only organizer"}]}}', "INSUFFICIENT_PERMISSIONS"],
    [403, '{"error":{"errors":[{"reason":"accessNotConfigured","message":"Google Calendar API has not been used in project 123"}]}}', "MISCONFIGURED"],
    [404, '{"error":{"errors":[{"reason":"notFound","message":"event not found"}]}}', "NOT_FOUND"],
    [410, '{"error":{"errors":[{"reason":"fullSyncRequired","message":"sync token expired"}]}}', "SYNC_CURSOR_ERROR"],
    [410, '{"error":{"errors":[{"reason":"updatedMinTooLongAgo","message":"too old"}]}}', "SYNC_CURSOR_ERROR"],
    [410, '{"error":{"errors":[{"reason":"deleted","message":"resource gone"}]}}', "NOT_FOUND"],
    [429, '{"error":{"errors":[{"reason":"rateLimitExceeded","message":"Retry after 2030-06-01T12:00:00Z"}]}}', "TEMPORARY_ERROR", true],
    [500, '{"error":{"errors":[{"reason":"backendError","message":"backend down"}]}}', "TEMPORARY_ERROR"],
    [502, '{"error":{"errors":[{"reason":"internal_failure","message":"internal"}]}}', "TEMPORARY_ERROR"],
    [503, '{"error":"unavailable"}', "TEMPORARY_ERROR"],
    [504, '{"error":{"errors":[{"reason":"backendError","message":"timeout"}]}}', "TEMPORARY_ERROR"],
    [418, 'teapot', "UNKNOWN"],
  ];

  for (const [status, body, expectedCode, hasRetry] of cases) {
    const reasonLabel = (() => { try { return JSON.parse(body)?.error?.errors?.[0]?.reason ?? "?"; } catch { return "?"; } })();
    it(`${status} reason=${reasonLabel} -> ${expectedCode}`, () => {
      const err = classifyHttp(status, body);
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
