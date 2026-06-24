import { describe, it, expect } from "bun:test";
import { parseIdTokenEmail } from "../lib/oauth";

function fakeIdToken(payload: Record<string, unknown>): string {
  const header = Buffer.from(JSON.stringify({ alg: "RS256" })).toString("base64url");
  const body = Buffer.from(JSON.stringify(payload)).toString("base64url");
  return `${header}.${body}.fakesig`;
}

describe("parseIdTokenEmail", () => {
  const cases: Array<[string, string | undefined, string | null]> = [
    ["valid email", fakeIdToken({ email: "sandro@getrootcx.com" }), "sandro@getrootcx.com"],
    ["uppercased email", fakeIdToken({ email: "Sandro@RootCX.com" }), "sandro@rootcx.com"],
    ["no email field in payload", fakeIdToken({ sub: "789" }), null],
    ["undefined input", undefined, null],
    ["empty string", "", null],
    ["malformed (no dots)", "nodots", null],
  ];

  for (const [label, input, expected] of cases) {
    it(label, () => {
      expect(parseIdTokenEmail(input)).toBe(expected);
    });
  }
});
