export interface ParsedAttendee {
  address: string;
  displayName: string;
  responseStatus: "needs_action" | "declined" | "tentative" | "accepted";
  isOrganizer: boolean;
  optional: boolean;
}

export interface ParsedEventAttachment {
  fileId: string;
  fileUrl: string;
  title: string;
  mimeType: string;
  iconLink: string;
}

export interface ParsedEvent {
  externalId: string;
  iCalUid: string;
  isCanceled: boolean;
  title: string;
  description: string;
  location: string;
  startsAt: number | null;
  endsAt: number | null;
  isFullDay: boolean;
  timeZone: string;
  externalCreatedAt: number | null;
  externalUpdatedAt: number | null;
  recurringEventExternalId: string;
  recurrence: string[];
  conferenceSolution: string;
  conferenceLink: string;
  htmlLink: string;
  organizerAddress: string;
  transparency: string;
  visibility: string;
  attendees: ParsedAttendee[];
  attachments: ParsedEventAttachment[];
}

const RESPONSE_MAP: Record<string, ParsedAttendee["responseStatus"]> = {
  accepted: "accepted",
  declined: "declined",
  tentative: "tentative",
  needsAction: "needs_action",
};

const toMs = (s?: string | null): number | null => {
  if (!s) return null;
  const t = Date.parse(s);
  return isFinite(t) ? t : null;
};

const DATE_ONLY_RE = /^(\d{4})-(\d{2})-(\d{2})$/;

const startMs = (when: any): number | null => {
  if (!when) return null;
  if (when.dateTime) return toMs(when.dateTime);
  if (!when.date) return null;
  const m = DATE_ONLY_RE.exec(when.date);
  if (!m) return toMs(when.date);
  return Date.UTC(Number(m[1]), Number(m[2]) - 1, Number(m[3]), 12, 0, 0);
};

export function parseEvent(e: any): ParsedEvent {
  const isFullDay = !e.start?.dateTime;
  const attendees: ParsedAttendee[] = (e.attendees ?? []).map((a: any) => ({
    address: (a.email ?? "").toLowerCase().trim(),
    displayName: a.displayName ?? "",
    responseStatus: RESPONSE_MAP[a.responseStatus] ?? "needs_action",
    isOrganizer: !!a.organizer,
    optional: !!a.optional,
  })).filter((a: ParsedAttendee) => a.address);

  const attachments: ParsedEventAttachment[] = (e.attachments ?? []).map((a: any) => ({
    fileId: a.fileId ?? "",
    fileUrl: a.fileUrl ?? "",
    title: a.title ?? "",
    mimeType: a.mimeType ?? "",
    iconLink: a.iconLink ?? "",
  }));

  const primaryEntry = e.conferenceData?.entryPoints?.find((p: any) => p.entryPointType === "video")
    ?? e.conferenceData?.entryPoints?.[0];

  return {
    externalId: e.id ?? "",
    iCalUid: e.iCalUID ?? "",
    isCanceled: e.status === "cancelled",
    title: e.summary ?? "",
    description: e.description ?? "",
    location: e.location ?? "",
    startsAt: startMs(e.start),
    endsAt: startMs(e.end),
    isFullDay,
    timeZone: e.start?.timeZone ?? e.end?.timeZone ?? "",
    externalCreatedAt: toMs(e.created),
    externalUpdatedAt: toMs(e.updated),
    recurringEventExternalId: e.recurringEventId ?? "",
    recurrence: Array.isArray(e.recurrence) ? e.recurrence : [],
    conferenceSolution: e.conferenceData?.conferenceSolution?.key?.type ?? "",
    conferenceLink: e.hangoutLink ?? primaryEntry?.uri ?? "",
    htmlLink: e.htmlLink ?? "",
    organizerAddress: (e.organizer?.email ?? "").toLowerCase(),
    transparency: e.transparency ?? "",
    visibility: e.visibility ?? "",
    attendees,
    attachments,
  };
}
