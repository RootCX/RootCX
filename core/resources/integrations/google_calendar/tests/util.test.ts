import { describe, it, expect } from "bun:test";
import { cronScheduleForUser } from "../lib/util";

describe("cronScheduleForUser", () => {
  it("produces valid cron with minute offset 0-4", () => {
    const sample = ["b7494ae3-f065-441a-a098-047e09470e7b", "00000000-0000-0000-0000-000000000001", "abc"];
    for (const id of sample) {
      const schedule = cronScheduleForUser(id);
      const match = schedule.match(/^(\d)-59\/5 \* \* \* \*$/);
      expect(match, id).not.toBeNull();
      const offset = Number(match![1]);
      expect(offset).toBeGreaterThanOrEqual(0);
      expect(offset).toBeLessThan(5);
    }
  });

  it("is deterministic for same userId", () => {
    const id = "user-xyz";
    expect(cronScheduleForUser(id)).toBe(cronScheduleForUser(id));
  });
});
