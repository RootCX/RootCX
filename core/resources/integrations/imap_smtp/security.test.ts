import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";

describe("IMAP/SMTP transport security", () => {
  test("certificate verification stays enabled for both transports", () => {
    const source = readFileSync(new URL("./index.ts", import.meta.url), "utf8");
    const disabled = source.match(/rejectUnauthorized\s*:\s*false/g) ?? [];
    const enabled = source.match(/rejectUnauthorized\s*:\s*true/g) ?? [];

    expect(disabled).toEqual([]);
    expect(enabled).toHaveLength(2);
    expect(source).toMatch(/requireTLS\s*:\s*true/);
  });
});
