import { describe, it, expect } from "bun:test";
import { composeEvent } from "../lib/compose-event";

describe("composeEvent", () => {
  const timedWhen = {
    start: { dateTime: "2026-04-01T10:00:00Z", timeZone: "UTC" },
    end:   { dateTime: "2026-04-01T11:00:00Z", timeZone: "UTC" },
  };

  it("minimal timed event -> conferenceDataVersion=0", () => {
    const r = composeEvent({ summary: "Test", ...timedWhen });
    expect(r.conferenceDataVersion).toBe(0);
    expect(r.body.summary).toBe("Test");
    expect(r.body.start.dateTime).toBe("2026-04-01T10:00:00Z");
    expect(r.body.conferenceData).toBeUndefined();
  });

  it("createMeet=true -> sets conferenceData.createRequest and conferenceDataVersion=1", () => {
    const r = composeEvent({ summary: "Meet", ...timedWhen, createMeet: true });
    expect(r.conferenceDataVersion).toBe(1);
    expect(r.body.conferenceData.createRequest.requestId).toBeTruthy();
    expect(r.body.conferenceData.createRequest.conferenceSolutionKey.type).toBe("hangoutsMeet");
  });

  it("all-day event uses date fields", () => {
    const r = composeEvent({
      summary: "Holiday",
      start: { date: "2026-12-25" },
      end:   { date: "2026-12-26" },
    });
    expect(r.body.start.date).toBe("2026-12-25");
    expect(r.body.end.date).toBe("2026-12-26");
  });

  it("attendees pass through normalized", () => {
    const r = composeEvent({
      summary: "Sync", ...timedWhen,
      attendees: [
        { email: "a@x.com" },
        { email: "b@y.com", displayName: "Bob", optional: true },
      ],
    });
    expect(r.body.attendees).toEqual([
      { email: "a@x.com" },
      { email: "b@y.com", displayName: "Bob", optional: true },
    ]);
  });

  it("recurrence passed through", () => {
    const r = composeEvent({
      summary: "Weekly", ...timedWhen,
      recurrence: ["RRULE:FREQ=WEEKLY;COUNT=10"],
    });
    expect(r.body.recurrence).toEqual(["RRULE:FREQ=WEEKLY;COUNT=10"]);
  });

  const invalidCases: Array<[string, Parameters<typeof composeEvent>[0]]> = [
    ["mixed all-day / timed", {
      summary: "Bad",
      start: { date: "2026-12-25" },
      end:   { dateTime: "2026-12-26T10:00:00Z" },
    }],
    ["missing summary", { summary: "", ...timedWhen }],
    ["start with both date and dateTime", {
      summary: "Bad",
      start: { date: "2026-12-25", dateTime: "2026-12-25T10:00:00Z" },
      end:   { dateTime: "2026-12-25T11:00:00Z" },
    }],
  ];

  for (const [label, input] of invalidCases) {
    it(`rejects: ${label}`, () => {
      expect(() => composeEvent(input)).toThrow();
    });
  }
});
