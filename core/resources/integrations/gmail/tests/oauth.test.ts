import { describe, it, expect } from "bun:test";
import { parseIdTokenEmail, oauth2ClientFor, evictClient } from "../lib/oauth";

function fakeIdToken(payload: Record<string, unknown>): string {
  const header = Buffer.from(JSON.stringify({ alg: "RS256" })).toString("base64url");
  const body = Buffer.from(JSON.stringify(payload)).toString("base64url");
  return `${header}.${body}.fakesig`;
}

describe("oauth2ClientFor cache isolation", () => {
  const config = { clientId: "id", clientSecret: "secret" };

  it("same user with different connections gets distinct clients", () => {
    evictClient("user1", "conn-a");
    evictClient("user1", "conn-b");
    const a = oauth2ClientFor(config, { refreshToken: "token-a" }, "user1", "conn-a");
    const b = oauth2ClientFor(config, { refreshToken: "token-b" }, "user1", "conn-b");
    expect(a).not.toBe(b);
  });

  it("same user and connection returns cached client", () => {
    evictClient("user2", "conn-x");
    const first = oauth2ClientFor(config, { refreshToken: "tok" }, "user2", "conn-x");
    const second = oauth2ClientFor(config, { refreshToken: "tok" }, "user2", "conn-x");
    expect(first).toBe(second);
  });
});

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
