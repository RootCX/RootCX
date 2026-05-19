import MailComposer from "nodemailer/lib/mail-composer";
import { extractMessageIdFromBuffer } from "./extract-message-id";

const MAX_MIME_BYTES = 35 * 1024 * 1024;

export interface SendInput {
  to: string | string[];
  cc?: string | string[];
  bcc?: string | string[];
  subject: string;
  text?: string;
  html?: string;
  attachments?: Array<{ filename: string; content: string; contentType?: string }>;
  inReplyTo?: string;
  references?: string | string[];
  threadId?: string;
  from?: string;
}

export interface ComposedMessage {
  rawBase64Url: string;
  headerMessageId: string;
}

/** Build a base64url-encoded RFC 5322 message ready for users.messages.send. */
export async function composeMessage(input: SendInput, defaultFrom: string): Promise<ComposedMessage> {
  const from = input.from ?? defaultFrom;
  const refs = input.references ?? input.inReplyTo;

  const options: any = {
    from,
    to: input.to,
    cc: input.cc,
    bcc: input.bcc,
    subject: input.subject,
    text: input.text,
    html: input.html,
  };
  if (input.attachments?.length) {
    options.attachments = input.attachments.map(a => ({
      filename: a.filename,
      content: Buffer.from(a.content, "base64"),
      contentType: a.contentType,
    }));
  }
  if (input.inReplyTo) options.inReplyTo = input.inReplyTo;
  if (refs) options.references = refs;

  const mail = new MailComposer(options);
  const compiled = mail.compile();
  (compiled as any).keepBcc = true;
  const buffer: Buffer = await compiled.build();

  if (buffer.length > MAX_MIME_BYTES) {
    const err: any = new Error(`message too large: ${buffer.length} > ${MAX_MIME_BYTES}`);
    err._tooLarge = true;
    throw err;
  }

  return {
    rawBase64Url: buffer.toString("base64url"),
    headerMessageId: extractMessageIdFromBuffer(buffer),
  };
}
