import { describe, it, expect } from "bun:test";
import { cryptoRandomId } from "../lib/util";

describe("cryptoRandomId", () => {
  it("returns unique IDs", () => {
    const a = cryptoRandomId();
    const b = cryptoRandomId();
    expect(a).not.toBe(b);
    expect(a.length).toBeGreaterThan(10);
  });
});
