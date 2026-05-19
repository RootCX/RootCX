import { describe, it, expect } from "bun:test";
import { parseMessage } from "../lib/parse-message";
import { extractMessageIdFromBuffer } from "../lib/extract-message-id";

describe("extractMessageIdFromBuffer", () => {
  const cases: Array<[string, string, string]> = [
    ["single line", "Message-ID: <abc@x.com>\r\nFrom: a@b.com\r\n\r\nbody", "<abc@x.com>"],
    ["folded continuation", "Message-ID:\r\n <folded@x.com>\r\nFrom: a@b.com\r\n\r\nbody", "<folded@x.com>"],
    ["absent", "From: a@b.com\r\nSubject: hi\r\n\r\nbody", ""],
    ["empty buffer", "", ""],
  ];

  for (const [label, raw, expected] of cases) {
    it(label, () => {
      expect(extractMessageIdFromBuffer(Buffer.from(raw))).toBe(expected);
    });
  }
});

describe("parseMessage", () => {
  function makeMsg(opts: {
    from?: string; to?: string; cc?: string; bcc?: string;
    subject?: string; msgId?: string; date?: string; inReplyTo?: string; references?: string;
    htmlBody?: string; textBody?: string; internalDate?: string;
    labelIds?: string[]; attachments?: any[];
  }) {
    const headers: any[] = [];
    if (opts.from) headers.push({ name: "From", value: opts.from });
    if (opts.to) headers.push({ name: "To", value: opts.to });
    if (opts.cc) headers.push({ name: "Cc", value: opts.cc });
    if (opts.bcc) headers.push({ name: "Bcc", value: opts.bcc });
    if (opts.subject) headers.push({ name: "Subject", value: opts.subject });
    if (opts.msgId) headers.push({ name: "Message-ID", value: opts.msgId });
    if (opts.date) headers.push({ name: "Date", value: opts.date });
    if (opts.inReplyTo) headers.push({ name: "In-Reply-To", value: opts.inReplyTo });
    if (opts.references) headers.push({ name: "References", value: opts.references });

    const parts: any[] = [];
    if (opts.htmlBody) {
      parts.push({
        mimeType: "text/html",
        body: { data: Buffer.from(opts.htmlBody).toString("base64url") },
      });
    }
    if (opts.textBody) {
      parts.push({
        mimeType: "text/plain",
        body: { data: Buffer.from(opts.textBody).toString("base64url") },
      });
    }
    if (opts.attachments) {
      for (const a of opts.attachments) parts.push(a);
    }

    return {
      id: "msg1", threadId: "th1", historyId: "h1",
      internalDate: opts.internalDate ?? "1700000000000",
      labelIds: opts.labelIds ?? ["INBOX"],
      snippet: "snip",
      payload: {
        mimeType: "multipart/alternative",
        headers,
        parts,
      },
    };
  }

  it("multipart with both text + html -> both populated", () => {
    const m = parseMessage(makeMsg({ htmlBody: "<b>hi</b>", textBody: "hi" }));
    expect(m.bodyHtml).toBe("<b>hi</b>");
    expect(m.bodyText).toBe("hi");
  });

  it("html only -> bodyText empty", () => {
    const m = parseMessage(makeMsg({ htmlBody: "<p>yo</p>" }));
    expect(m.bodyHtml).toBe("<p>yo</p>");
    expect(m.bodyText).toBe("");
  });

  it("text only -> bodyHtml empty", () => {
    const m = parseMessage(makeMsg({ textBody: "plain" }));
    expect(m.bodyText).toBe("plain");
    expect(m.bodyHtml).toBe("");
  });

  it("recipients with mixed formats parsed correctly", () => {
    const m = parseMessage(makeMsg({
      from: "Alice <alice@x.com>",
      to: 'Bob <bob@y.com>, plain@z.com, "Charlie, Jr" <charlie@z.com>',
    }));
    expect(m.from).toEqual({ name: "Alice", address: "alice@x.com" });
    expect(m.to).toHaveLength(3);
    expect(m.to[0]).toEqual({ name: "Bob", address: "bob@y.com" });
    expect(m.to[1].address).toBe("plain@z.com");
    expect(m.to[2]).toEqual({ name: "Charlie, Jr", address: "charlie@z.com" });
  });

  it("missing headerMessageId adds parseWarning", () => {
    const m = parseMessage(makeMsg({ from: "a@b.com", to: "c@d.com" }));
    expect(m.headerMessageId).toBe("");
    expect(m.parseWarnings).toContain("missing-headerMessageId");
  });

  it("missing internalDate -> uses Date header", () => {
    const m = parseMessage({
      id: "x", threadId: "t", historyId: "h",
      internalDate: null,
      labelIds: [],
      snippet: "",
      payload: {
        mimeType: "text/plain",
        headers: [
          { name: "Date", value: "Mon, 01 Jan 2024 12:00:00 +0000" },
          { name: "From", value: "a@b.com" },
        ],
        body: { data: Buffer.from("hello").toString("base64url") },
      },
    });
    expect(m.date).toBe("2024-01-01T12:00:00.000Z");
    expect(m.parseWarnings).toContain("missing-internalDate");
  });

  it("attachments listed with correct fields", () => {
    const m = parseMessage(makeMsg({
      from: "a@b.com",
      textBody: "see attachment",
      attachments: [{
        filename: "doc.pdf",
        mimeType: "application/pdf",
        body: { attachmentId: "att123", size: 42000 },
        headers: [
          { name: "Content-Disposition", value: "attachment; filename=doc.pdf" },
        ],
      }],
    }));
    expect(m.attachments).toHaveLength(1);
    expect(m.attachments[0].id).toBe("att123");
    expect(m.attachments[0].filename).toBe("doc.pdf");
    expect(m.attachments[0].mimeType).toBe("application/pdf");
    expect(m.attachments[0].size).toBe(42000);
    expect(m.attachments[0].isInline).toBe(false);
  });

  it("inline image detected via Content-ID", () => {
    const m = parseMessage(makeMsg({
      from: "a@b.com",
      htmlBody: "<img src='cid:img1'>",
      attachments: [{
        filename: "",
        mimeType: "image/png",
        body: { attachmentId: "att_inline", size: 1024 },
        headers: [
          { name: "Content-ID", value: "<img1>" },
          { name: "Content-Disposition", value: "inline" },
        ],
      }],
    }));
    expect(m.attachments).toHaveLength(1);
    expect(m.attachments[0].isInline).toBe(true);
    expect(m.attachments[0].contentId).toBe("img1");
  });
});
