import { cryptoRandomId } from "./util";

export interface ComposeInput {
  summary: string;
  description?: string;
  location?: string;
  start: { dateTime?: string; date?: string; timeZone?: string };
  end:   { dateTime?: string; date?: string; timeZone?: string };
  attendees?: Array<{ email: string; displayName?: string; optional?: boolean }>;
  recurrence?: string[];
  createMeet?: boolean;
  guestsCanModify?: boolean;
  guestsCanInviteOthers?: boolean;
  transparency?: "opaque" | "transparent";
  visibility?: "default" | "public" | "private" | "confidential";
}

export interface ComposedEvent {
  body: Record<string, any>;
  conferenceDataVersion: 0 | 1;
}

const isAllDay = (w: ComposeInput["start"]) => !!w.date && !w.dateTime;

const validateWhen = (w: ComposeInput["start"], field: "start" | "end") => {
  if (w.date && w.dateTime) throw new Error(`${field}: date and dateTime are mutually exclusive`);
  if (!w.date && !w.dateTime) throw new Error(`${field}: date or dateTime required`);
};

export function composeEvent(input: ComposeInput): ComposedEvent {
  if (!input.summary) throw new Error("summary required");
  validateWhen(input.start, "start");
  validateWhen(input.end, "end");
  if (isAllDay(input.start) !== isAllDay(input.end)) {
    throw new Error("start and end must both be all-day or both timed");
  }

  const body: Record<string, any> = {
    summary: input.summary,
    start: input.start,
    end: input.end,
  };
  if (input.description !== undefined) body.description = input.description;
  if (input.location !== undefined) body.location = input.location;
  if (input.attendees?.length) body.attendees = input.attendees.map(a => ({
    email: a.email,
    ...(a.displayName ? { displayName: a.displayName } : {}),
    ...(a.optional ? { optional: true } : {}),
  }));
  if (input.recurrence?.length) body.recurrence = input.recurrence;
  if (input.transparency) body.transparency = input.transparency;
  if (input.visibility) body.visibility = input.visibility;
  if (input.guestsCanModify !== undefined) body.guestsCanModify = input.guestsCanModify;
  if (input.guestsCanInviteOthers !== undefined) body.guestsCanInviteOthers = input.guestsCanInviteOthers;

  let conferenceDataVersion: 0 | 1 = 0;
  if (input.createMeet) {
    body.conferenceData = {
      createRequest: {
        requestId: cryptoRandomId(),
        conferenceSolutionKey: { type: "hangoutsMeet" },
      },
    };
    conferenceDataVersion = 1;
  }

  return { body, conferenceDataVersion };
}
