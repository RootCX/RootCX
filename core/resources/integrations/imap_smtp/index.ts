/// <reference path="../rootcx-worker.d.ts" />
import { ImapFlow } from "imapflow";
import { createTransport } from "nodemailer";
import PostalMime from "postal-mime";

interface Creds {
  imapHost: string;
  imapPort: number;
  smtpHost: string;
  smtpPort: number;
  username: string;
  password: string;
  secure?: boolean;
}

async function withImap<T>(creds: Creds, fn: (client: ImapFlow) => Promise<T>): Promise<T> {
  const client = new ImapFlow({
    host: creds.imapHost,
    port: creds.imapPort,
    secure: creds.secure ?? true,
    auth: { user: creds.username, pass: creds.password },
    logger: false,
    tls: { rejectUnauthorized: false },
    greetingTimeout: 16_000,
    socketTimeout: 30_000,
  });
  await client.connect();
  try {
    return await fn(client);
  } finally {
    await client.logout().catch(() => {});
  }
}

function smtpTransport(creds: Creds) {
  const port = Number(creds.smtpPort);
  const secure = port === 465;
  return createTransport({
    host: creds.smtpHost,
    port,
    secure,
    auth: { user: creds.username, pass: creds.password },
    tls: { rejectUnauthorized: false },
    connectionTimeout: 10_000,
    greetingTimeout: 10_000,
    socketTimeout: 20_000,
  });
}

function formatAddr(addr: { name?: string; address?: string } | undefined): string {
  if (!addr) return "";
  return addr.name ? `${addr.name} <${addr.address}>` : addr.address ?? "";
}

function formatAddrList(list: Array<{ name?: string; address?: string }> | undefined): string {
  if (!list?.length) return "";
  return list.map(formatAddr).join(", ");
}

async function parseMessage(source: Buffer | Uint8Array, uid: number, flags: string[]) {
  const parsed = await PostalMime.parse(source);
  return {
    uid,
    headerMessageId: parsed.messageId ?? null,
    from: formatAddr(parsed.from),
    to: formatAddrList(parsed.to),
    cc: formatAddrList(parsed.cc),
    subject: parsed.subject ?? "",
    date: parsed.date ? new Date(parsed.date).toISOString() : "",
    body: parsed.text ?? parsed.html ?? "",
    flags,
    threadId: parsed.inReplyTo ?? parsed.messageId ?? "",
    labelIds: flags.includes("\\Seen") ? [] : ["UNREAD"],
  };
}

// ─── Actions ────────────────────────────────────────────────────────────────

async function listEmails(_config: any, creds: Creds, input: any) {
  const { folder = "INBOX", sinceUid, maxResults = 500 } = input ?? {};

  return withImap(creds, async (client) => {
    const lock = await client.getMailboxLock(folder);
    try {
      const uidValidity = Number(client.mailbox?.uidValidity ?? 0);
      const uidNext = Number(client.mailbox?.uidNext ?? 1);
      const highestUid = Math.max(0, uidNext - 1);

      if (highestUid === 0) {
        return { messageUids: [], uidValidity, highestUid };
      }

      const startUid = sinceUid ? sinceUid + 1 : 1;
      if (startUid > highestUid) {
        return { messageUids: [], uidValidity, highestUid };
      }

      const uidRange = `${startUid}:${highestUid}`;
      const uids = await client.search({ uid: uidRange }, { uid: true });
      const result = Array.isArray(uids) ? uids.slice(0, maxResults) : [];
      return { messageUids: result, uidValidity, highestUid };
    } finally {
      lock.release();
    }
  });
}

async function getEmail(_config: any, creds: Creds, input: any) {
  const { uid, folder = "INBOX" } = input;

  return withImap(creds, async (client) => {
    const lock = await client.getMailboxLock(folder);
    try {
      const msg = await client.fetchOne(String(uid), { source: true, flags: true }, { uid: true });
      if (!msg?.source) throw new Error(`message uid=${uid} not found`);
      return parseMessage(msg.source, uid, [...msg.flags]);
    } finally {
      lock.release();
    }
  });
}

async function batchGetEmails(_config: any, creds: Creds, input: any) {
  const { uids, folder = "INBOX" } = input;
  if (!uids?.length) return { messages: [] };

  return withImap(creds, async (client) => {
    const lock = await client.getMailboxLock(folder);
    try {
      const uidSet = uids.join(",");
      const raw: Array<{ source: Buffer | Uint8Array; uid: number; flags: string[] }> = [];

      for await (const msg of client.fetch(uidSet, { source: true, flags: true, uid: true }, { uid: true })) {
        if (!msg.source) continue;
        raw.push({ source: msg.source, uid: msg.uid, flags: [...msg.flags] });
      }

      const messages = await Promise.all(raw.map(m => parseMessage(m.source, m.uid, m.flags)));
      return { messages };
    } finally {
      lock.release();
    }
  });
}

async function sendEmail(_config: any, creds: Creds, input: any) {
  const { to, subject, body, cc, bcc, html } = input;
  log.info(`SMTP connecting to ${creds.smtpHost}:${creds.smtpPort} (secure=${creds.secure})`);

  const transport = smtpTransport(creds);
  const mailOptions: any = {
    from: creds.username,
    to,
    subject,
    ...(cc ? { cc } : {}),
    ...(bcc ? { bcc } : {}),
    ...(html ? { html: body } : { text: body }),
  };

  const info = await transport.sendMail(mailOptions);
  const messageId = info.messageId ?? "";

  // Append to Sent folder via IMAP
  try {
    const MailComposer = (await import("nodemailer/lib/mail-composer")).default;
    const composer = new MailComposer(mailOptions);
    const message: Buffer = await new Promise((resolve, reject) => {
      composer.compile().build((err: Error | null, buf: Buffer) => {
        if (err) reject(err); else resolve(buf);
      });
    });

    await withImap(creds, async (client) => {
      const sentFolder = await findSentFolder(client);
      if (!sentFolder) return;
      await client.append(sentFolder, message, ["\\Seen"]);
    });
  } catch (e: any) {
    log.warn(`append to Sent failed: ${e.message}`);
  }

  return { messageId, ok: true };
}

async function modifyEmail(_config: any, creds: Creds, input: any) {
  const { uid, folder = "INBOX", addFlags, removeFlags } = input;

  return withImap(creds, async (client) => {
    const lock = await client.getMailboxLock(folder);
    try {
      if (addFlags?.length) {
        await client.messageFlagsAdd(String(uid), addFlags, { uid: true });
      }
      if (removeFlags?.length) {
        await client.messageFlagsRemove(String(uid), removeFlags, { uid: true });
      }
      return { ok: true };
    } finally {
      lock.release();
    }
  });
}

async function getFolders(_config: any, creds: Creds, _input: any) {
  return withImap(creds, async (client) => {
    const list = await client.list();
    const folders = list
      .filter((f: any) => !f.flags?.has("\\Noselect"))
      .map((f: any) => ({
        path: f.path,
        name: f.name,
        specialUse: f.specialUse ?? null,
        flags: [...(f.flags ?? [])],
      }));
    return { folders };
  });
}

async function findSentFolder(client: ImapFlow): Promise<string | null> {
  const list = await client.list();
  const sent = list.find((f: any) => f.specialUse === "\\Sent");
  if (sent) return sent.path;
  const byName = list.find((f: any) => /^(sent|sent items|sent mail)$/i.test(f.name));
  return byName?.path ?? null;
}

// ─── RPC handlers ───────────────────────────────────────────────────────────

const actions: Record<string, (c: any, u: Creds, i: any) => Promise<any>> = {
  send_email: sendEmail,
  list_emails: listEmails,
  get_email: getEmail,
  batch_get_emails: batchGetEmails,
  modify_email: modifyEmail,
  get_folders: getFolders,
};

serve({
  rpc: {
    async __auth_start(_params) {
      return {
        type: "credentials",
        schema: {
          type: "object",
          properties: {
            imapHost: { type: "string", label: "IMAP Host", placeholder: "imap.gmail.com" },
            imapPort: { type: "integer", label: "IMAP Port", placeholder: "993", default: 993 },
            smtpHost: { type: "string", label: "SMTP Host", placeholder: "smtp.gmail.com" },
            smtpPort: { type: "integer", label: "SMTP Port", placeholder: "465", default: 465 },
            username: { type: "string", label: "Email / Username", placeholder: "you@example.com" },
            password: { type: "string", label: "Password", placeholder: "App-specific password", secret: true },
            secure: { type: "boolean", label: "Use TLS", default: true },
          },
          required: ["imapHost", "imapPort", "smtpHost", "smtpPort", "username", "password"],
        },
      };
    },

    async __auth_callback(_params) {
      return { credentials: {} };
    },

    async __integration(params) {
      const { action, input, userCredentials } = params;
      if (!userCredentials?.imapHost || !userCredentials?.username) {
        throw new Error("not connected — credentials required");
      }
      const handler = actions[action];
      if (!handler) throw new Error(`unknown action: ${action}`);
      return handler(null, userCredentials as Creds, input);
    },
  },
});
