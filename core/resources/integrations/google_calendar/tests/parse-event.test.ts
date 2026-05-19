import { describe, it, expect } from "bun:test";
import { parseEvent } from "../lib/parse-event";

const baseEvent = (overrides: any = {}) => ({
  id: "evt1",
  iCalUID: "evt1@google.com",
  status: "confirmed",
  htmlLink: "https://calendar.google.com/event?eid=evt1",
  summary: "Sync meeting",
  description: "weekly",
  location: "HQ",
  created: "2026-01-01T10:00:00.000Z",
  updated: "2026-01-02T11:00:00.000Z",
  start: { dateTime: "2026-01-15T14:00:00Z", timeZone: "UTC" },
  end:   { dateTime: "2026-01-15T15:00:00Z", timeZone: "UTC" },
  organizer: { email: "Org@Example.COM", displayName: "Org", self: true },
  attendees: [
    { email: "Alice@X.COM", displayName: "Alice", responseStatus: "accepted", organizer: false, self: false },
    { email: "bob@y.com",   responseStatus: "needsAction", optional: true },
  ],
  ...overrides,
});

describe("parseEvent", () => {
  it("timed event extracts core fields", () => {
    const e = parseEvent(baseEvent());
    expect(e.externalId).toBe("evt1");
    expect(e.iCalUid).toBe("evt1@google.com");
    expect(e.isCanceled).toBe(false);
    expect(e.title).toBe("Sync meeting");
    expect(e.isFullDay).toBe(false);
    expect(e.startsAt).toBe(Date.parse("2026-01-15T14:00:00Z"));
    expect(e.endsAt).toBe(Date.parse("2026-01-15T15:00:00Z"));
    expect(e.timeZone).toBe("UTC");
    expect(e.htmlLink).toContain("calendar.google.com");
    expect(e.organizerAddress).toBe("org@example.com");
  });

  it("attendees normalized: lowercase, response_status mapped", () => {
    const e = parseEvent(baseEvent());
    expect(e.attendees).toHaveLength(2);
    expect(e.attendees[0].address).toBe("alice@x.com");
    expect(e.attendees[0].responseStatus).toBe("accepted");
    expect(e.attendees[1].responseStatus).toBe("needs_action");
    expect(e.attendees[1].optional).toBe(true);
  });

  it("all-day event stored at noon UTC to survive timezone shifts", () => {
    const e = parseEvent(baseEvent({
      start: { date: "2026-03-10" },
      end:   { date: "2026-03-11" },
    }));
    expect(e.isFullDay).toBe(true);
    expect(e.startsAt).toBe(Date.UTC(2026, 2, 10, 12, 0, 0));
    expect(e.endsAt).toBe(Date.UTC(2026, 2, 11, 12, 0, 0));
  });

  it("cancelled event sets isCanceled=true", () => {
    const e = parseEvent(baseEvent({ status: "cancelled" }));
    expect(e.isCanceled).toBe(true);
  });

  it("recurring event instance carries recurringEventId", () => {
    const e = parseEvent(baseEvent({ recurringEventId: "parent123" }));
    expect(e.recurringEventExternalId).toBe("parent123");
  });

  it("recurrence rules preserved as array", () => {
    const e = parseEvent(baseEvent({ recurrence: ["RRULE:FREQ=WEEKLY;BYDAY=MO"] }));
    expect(e.recurrence).toEqual(["RRULE:FREQ=WEEKLY;BYDAY=MO"]);
  });

  it("Google Meet conference data extracted from hangoutLink", () => {
    const e = parseEvent(baseEvent({
      hangoutLink: "https://meet.google.com/abc-defg-hij",
      conferenceData: {
        conferenceSolution: { key: { type: "hangoutsMeet" } },
        entryPoints: [
          { entryPointType: "video", uri: "https://meet.google.com/abc-defg-hij" },
          { entryPointType: "phone", uri: "tel:+1-555-0123" },
        ],
      },
    }));
    expect(e.conferenceSolution).toBe("hangoutsMeet");
    expect(e.conferenceLink).toBe("https://meet.google.com/abc-defg-hij");
  });

  it("falls back to first entry point when no hangoutLink", () => {
    const e = parseEvent(baseEvent({
      conferenceData: {
        conferenceSolution: { key: { type: "addOn" } },
        entryPoints: [{ entryPointType: "video", uri: "https://example.zoom.us/j/123" }],
      },
    }));
    expect(e.conferenceLink).toBe("https://example.zoom.us/j/123");
  });

  it("attachments preserved with all fields", () => {
    const e = parseEvent(baseEvent({
      attachments: [{
        fileId: "drive123", fileUrl: "https://drive.google.com/file/d/drive123",
        title: "Agenda.pdf", mimeType: "application/pdf",
        iconLink: "https://drive-icon.png",
      }],
    }));
    expect(e.attachments).toHaveLength(1);
    expect(e.attachments[0].fileId).toBe("drive123");
    expect(e.attachments[0].title).toBe("Agenda.pdf");
  });

  it("missing optional fields handled gracefully", () => {
    const e = parseEvent({ id: "evt2", iCalUID: "evt2@x", status: "confirmed" });
    expect(e.title).toBe("");
    expect(e.attendees).toEqual([]);
    expect(e.attachments).toEqual([]);
    expect(e.startsAt).toBeNull();
    expect(e.endsAt).toBeNull();
  });

  it("organizer flag propagates", () => {
    const e = parseEvent(baseEvent({
      attendees: [{ email: "me@x.com", responseStatus: "accepted", organizer: true }],
    }));
    expect(e.attendees[0].isOrganizer).toBe(true);
  });

  it("attendee with no email is filtered out", () => {
    const e = parseEvent(baseEvent({
      attendees: [
        { email: "valid@x.com", responseStatus: "accepted" },
        { displayName: "Resource only", responseStatus: "accepted" },
      ],
    }));
    expect(e.attendees).toHaveLength(1);
    expect(e.attendees[0].address).toBe("valid@x.com");
  });
});
