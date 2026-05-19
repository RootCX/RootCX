import addressparser from "nodemailer/lib/addressparser";

export interface ParsedAddress { name: string; address: string }
export interface ParsedAttachment {
  id: string;
  filename: string;
  mimeType: string;
  size: number;
  contentId: string | null;
  isInline: boolean;
}

export interface ParsedMessage {
  id: string;
  threadId: string;
  historyId: string | null;
  headerMessageId: string;
  internalDate: number;
  from: ParsedAddress | null;
  to: ParsedAddress[];
  cc: ParsedAddress[];
  bcc: ParsedAddress[];
  replyTo: ParsedAddress[];
  deliveredTo: ParsedAddress[];
  inReplyTo: string | null;
  references: string[];
  subject: string;
  date: string;
  snippet: string;
  labelIds: string[];
  bodyHtml: string;
  bodyText: string;
  attachments: ParsedAttachment[];
  parseWarnings: string[];
}

function header(headers: any[], name: string): string {
  const n = name.toLowerCase();
  return headers.find((h: any) => h.name?.toLowerCase() === n)?.value ?? "";
}

function parseAddrs(raw: string): ParsedAddress[] {
  if (!raw) return [];
  try {
    const list = (addressparser as any)(raw) as Array<{ address?: string; name?: string }>;
    return list
      .filter(a => a.address)
      .map(a => ({ name: (a.name ?? "").trim(), address: a.address!.toLowerCase().trim() }));
  } catch {
    return [];
  }
}

function firstAddr(raw: string): ParsedAddress | null {
  const list = parseAddrs(raw);
  return list[0] ?? null;
}

function decodeB64Url(data: string): string {
  return Buffer.from(data, "base64url").toString("utf-8");
}

interface Bodies { html: string; text: string }

function walkBodies(payload: any, out: Bodies): void {
  if (!payload) return;
  const mime = (payload.mimeType ?? "").toLowerCase();
  if (mime === "text/html") {
    if (payload.body?.data && !out.html) out.html = decodeB64Url(payload.body.data);
    return;
  }
  if (mime === "text/plain") {
    if (payload.body?.data && !out.text) out.text = decodeB64Url(payload.body.data);
    return;
  }
  if (mime.startsWith("multipart/") && Array.isArray(payload.parts)) {
    for (const p of payload.parts) walkBodies(p, out);
  }
}

function collectAttachments(payload: any, acc: ParsedAttachment[]): void {
  if (!payload) return;
  const filename = payload.filename ?? "";
  const attachmentId = payload.body?.attachmentId;
  if (attachmentId && (filename || /\bimage\//i.test(payload.mimeType ?? ""))) {
    const cidHeader = (payload.headers ?? []).find((h: any) => h.name?.toLowerCase() === "content-id")?.value ?? "";
    const dispHeader = (payload.headers ?? []).find((h: any) => h.name?.toLowerCase() === "content-disposition")?.value ?? "";
    const contentId = cidHeader.replace(/^<|>$/g, "") || null;
    acc.push({
      id: attachmentId,
      filename,
      mimeType: payload.mimeType ?? "application/octet-stream",
      size: payload.body?.size ?? 0,
      contentId,
      isInline: /inline/i.test(dispHeader) || (!!contentId && !filename),
    });
  }
  if (Array.isArray(payload.parts)) for (const p of payload.parts) collectAttachments(p, acc);
}

export function parseMessage(msg: any): ParsedMessage {
  const headers = msg.payload?.headers ?? [];
  const warnings: string[] = [];

  const rawFrom = header(headers, "From");
  const from = rawFrom ? firstAddr(rawFrom) : null;
  if (!from) warnings.push("missing-from");

  const to = parseAddrs(header(headers, "To"));
  const cc = parseAddrs(header(headers, "Cc"));
  const bcc = parseAddrs(header(headers, "Bcc"));
  const replyTo = parseAddrs(header(headers, "Reply-To"));
  const deliveredTo = parseAddrs(header(headers, "Delivered-To"));

  const headerMessageId = header(headers, "Message-ID") || header(headers, "Message-Id") || "";
  if (!headerMessageId) warnings.push("missing-headerMessageId");
  if (!msg.historyId) warnings.push("missing-historyId");

  const inReplyTo = header(headers, "In-Reply-To") || null;
  const referencesRaw = header(headers, "References");
  const references = referencesRaw ? referencesRaw.split(/\s+/).filter(Boolean) : [];

  const rawInternal = msg.internalDate;
  const internalDate = rawInternal ? Number(rawInternal) : 0;
  if (!internalDate) warnings.push("missing-internalDate");

  const dateHeader = header(headers, "Date");
  let date = "";
  if (dateHeader) {
    const parsed = new Date(dateHeader);
    date = isNaN(parsed.getTime()) ? dateHeader : parsed.toISOString();
  } else if (internalDate) {
    date = new Date(internalDate).toISOString();
  }

  const bodies: Bodies = { html: "", text: "" };
  walkBodies(msg.payload, bodies);

  const attachments: ParsedAttachment[] = [];
  collectAttachments(msg.payload, attachments);

  return {
    id: msg.id ?? "",
    threadId: msg.threadId ?? "",
    historyId: msg.historyId ?? null,
    headerMessageId,
    internalDate,
    from,
    to, cc, bcc, replyTo, deliveredTo,
    inReplyTo,
    references,
    subject: header(headers, "Subject"),
    date,
    snippet: msg.snippet ?? "",
    labelIds: msg.labelIds ?? [],
    bodyHtml: bodies.html,
    bodyText: bodies.text,
    attachments,
    parseWarnings: warnings,
  };
}
