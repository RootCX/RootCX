/// <reference path="../rootcx-worker.d.ts" />
import { ImapFlow } from "imapflow";
import { createTransport } from "nodemailer";
import MailComposer from "nodemailer/lib/mail-composer";
import PostalMime from "postal-mime";

const MAX_THROTTLE = 5;
const FETCH_CONCURRENCY = 10;
const SKIP_SPECIAL_USE = new Set(["\\Junk", "\\Trash", "\\Drafts"]);

interface Creds { imapHost: string; imapPort: number; smtpHost: string; username: string; password: string }

let ctx: RootCxCtx;
let db: any = null;

serve({
  async onStart(c) {
    ctx = c;
    const postgres = (await import("postgres")).default;
    db = postgres(c.databaseUrl, { max: 10, idle_timeout: 30 });
    ensureIndexes().catch(e => log.error(`indexes: ${e.message}`));
  },
  rpc: {
    async __auth_start() {
      return {
        type: "credentials",
        schema: {
          type: "object",
          properties: {
            username: { type: "string", label: "Email / Username", placeholder: "you@example.com" },
            password: { type: "string", label: "Password", placeholder: "App-specific password", secret: true },
            imapHost: { type: "string", label: "IMAP Host", placeholder: "imap.example.com" },
            imapPort: { type: "integer", label: "IMAP Port", placeholder: "993", default: 993 },
            smtpHost: { type: "string", label: "SMTP Host", placeholder: "smtp.example.com" },
          },
          required: ["username", "password", "imapHost", "smtpHost"],
        },
      };
    },
    async __auth_callback() { return { credentials: {} }; },
    async __integration(params, caller) {
      const { action, input, userCredentials, userId } = params;
      if (!userCredentials?.imapHost || !userCredentials?.username) {
        return { ok: false, error: { code: "INSUFFICIENT_PERMISSIONS", message: "not connected" } };
      }
      const handler = actions[action];
      if (!handler) return { ok: false, error: { code: "MISCONFIGURED", message: `unknown action: ${action}` } };
      const token = caller?.authToken ?? "";
      return handler(userCredentials as Creds, input, userId, token);
    },
  },
  onJob: handleJob,
});

async function ensureIndexes() {
  if (!db) return;
  await db`CREATE UNIQUE INDEX IF NOT EXISTS idx_imap_msg_ext ON imap_smtp.messages (external_id)`;
  await db`CREATE UNIQUE INDEX IF NOT EXISTS idx_imap_msg_hdr ON imap_smtp.messages (header_message_id)`;
  await db`CREATE INDEX IF NOT EXISTS idx_imap_msg_thread ON imap_smtp.messages (thread_external_id)`;
  await db`CREATE INDEX IF NOT EXISTS idx_imap_msg_user ON imap_smtp.messages (user_id)`;
  await db`CREATE UNIQUE INDEX IF NOT EXISTS idx_imap_thread_ext ON imap_smtp.threads (external_id)`;
  await db`CREATE INDEX IF NOT EXISTS idx_imap_part_msg ON imap_smtp.participants (message_id)`;
  await db`CREATE INDEX IF NOT EXISTS idx_imap_part_addr ON imap_smtp.participants (lower(address))`;
  await db`CREATE INDEX IF NOT EXISTS idx_imap_att_msg ON imap_smtp.attachments (message_id)`;
  await db`CREATE UNIQUE INDEX IF NOT EXISTS idx_imap_cursor_user ON imap_smtp.sync_cursors (user_id)`;
}

async function withImap<T>(creds: Creds, fn: (client: ImapFlow) => Promise<T>): Promise<T> {
  const client = new ImapFlow({
    host: creds.imapHost, port: creds.imapPort, secure: true,
    auth: { user: creds.username, pass: creds.password },
    logger: false, tls: { rejectUnauthorized: false },
    greetingTimeout: 16_000, socketTimeout: 30_000,
  });
  await client.connect();
  try { return await fn(client); }
  finally { await client.logout().catch(() => {}); }
}

async function sendEmail(creds: Creds, input: any, userId: string, token: string) {
  const { to, subject, body, cc, bcc, html } = input;
  const transport = createTransport({
    host: creds.smtpHost, port: 587, secure: false,
    auth: { user: creds.username, pass: creds.password },
    tls: { rejectUnauthorized: false },
  });
  const mailOptions: any = {
    from: creds.username, to, subject,
    ...(cc ? { cc } : {}), ...(bcc ? { bcc } : {}),
    ...(html ? { html: body } : { text: body }),
  };
  const info = await transport.sendMail(mailOptions);
  const messageId = info.messageId ?? "";

  try {
    const composer = new MailComposer(mailOptions);
    const compiled = composer.compile();
    const message: Buffer = await compiled.build();
    await withImap(creds, async (client) => {
      const sentFolder = await findSentFolder(client);
      if (sentFolder) await client.append(sentFolder, message, ["\\Seen"]);
    });
  } catch (e: any) { log.warn(`append to Sent: ${e.message}`); }

  // Auto-persist
  if (token) {
    try {
      const parsed = await selfAction(token, "get_email", { uid: 0, folder: "SENT", messageId });
      if (parsed) await persistParsedMessage(userId, parsed);
    } catch { /* will be picked up on next sync */ }
  }

  return { ok: true, data: { messageId, ok: true } };
}

async function getEmail(creds: Creds, input: any, _userId: string, _token: string) {
  const { uid, folder = "INBOX" } = input;
  const msg = await withImap(creds, async (client) => {
    const lock = await client.getMailboxLock(folder);
    try {
      const fetched = await client.fetchOne(String(uid), { source: true, flags: true }, { uid: true });
      if (!fetched?.source) throw new Error(`uid=${uid} not found`);
      return { source: fetched.source, flags: [...fetched.flags] };
    } finally { lock.release(); }
  });
  return { ok: true, data: await parseImapMessage(msg.source, uid, folder, msg.flags) };
}

async function getFolders(creds: Creds, _input: any, _userId: string, _token: string) {
  const folders = await withImap(creds, async (client) => {
    const list = await client.list();
    return list
      .filter((f: any) => !f.flags?.has("\\Noselect"))
      .map((f: any) => ({ path: f.path, name: f.name, specialUse: f.specialUse ?? null, flags: [...(f.flags ?? [])] }));
  });
  return { ok: true, data: { folders } };
}

async function syncConnect(creds: Creds, _input: any, userId: string, token: string) {
  const handle = creds.username.toLowerCase();
  const existing = await db`SELECT id, cron_id FROM imap_smtp.sync_cursors WHERE user_id = ${userId}`;
  let cursorId: string;
  let cronId: string | null = null;
  if (existing.length) {
    cursorId = existing[0].id;
    cronId = existing[0].cron_id;
    await db`UPDATE imap_smtp.sync_cursors SET handle = ${handle}, enabled = true, status = 'idle', throttle_count = 0, throttle_after = null WHERE id = ${cursorId}`;
  } else {
    const ins = await db`INSERT INTO imap_smtp.sync_cursors (user_id, handle, status, enabled, throttle_count) VALUES (${userId}, ${handle}, 'idle', true, 0) RETURNING id`;
    cursorId = ins[0].id;
  }
  if (!cronId && token) {
    cronId = await createSyncCron(userId, token);
    if (cronId) await db`UPDATE imap_smtp.sync_cursors SET cron_id = ${cronId} WHERE id = ${cursorId}`;
  }
  if (token) await dispatchSyncJob(userId, token);
  return { ok: true, data: { cursor_id: cursorId, handle } };
}

async function syncDisconnect(_creds: Creds, _input: any, userId: string, token: string) {
  const cursor = await db`SELECT cron_id FROM imap_smtp.sync_cursors WHERE user_id = ${userId}`;
  if (cursor.length && cursor[0].cron_id && token) {
    await deleteSyncCron(cursor[0].cron_id, token);
  }
  await db`UPDATE imap_smtp.sync_cursors SET enabled = false, cron_id = null WHERE user_id = ${userId}`;
  return { ok: true, data: {} };
}

async function syncNow(_creds: Creds, _input: any, userId: string, token: string) {
  const cursor = await db`SELECT id, status, throttle_after FROM imap_smtp.sync_cursors WHERE user_id = ${userId} AND enabled = true`;
  if (!cursor.length) return { ok: false, error: { code: "MISCONFIGURED", message: "no active sync" } };
  if (cursor[0].status === "syncing") return { ok: true, data: { triggered: false } };
  if (cursor[0].throttle_after && new Date(cursor[0].throttle_after).getTime() > Date.now()) return { ok: true, data: { triggered: false } };
  if (token) await dispatchSyncJob(userId, token);
  return { ok: true, data: { triggered: true } };
}

const actions: Record<string, (creds: Creds, input: any, userId: string, token: string) => Promise<any>> = {
  send_email: sendEmail, get_email: getEmail, get_folders: getFolders,
  sync_connect: syncConnect, sync_disconnect: syncDisconnect, sync_now: syncNow,
};

async function handleJob(payload: any, caller: any) {
  if (payload?.type !== "sync") return { skipped: true };
  const token: string = caller?.authToken;
  if (!token) return { error: "no auth token in job caller" };
  const userId = payload.user_id;
  const cursors = await db`SELECT * FROM imap_smtp.sync_cursors WHERE user_id = ${userId} AND enabled = true`;
  if (!cursors.length) return { skipped: true };
  const sc = cursors[0];

  try {
    await db`UPDATE imap_smtp.sync_cursors SET status = 'syncing' WHERE id = ${sc.id}`;
    await runSync(token, userId, sc);
    await db`UPDATE imap_smtp.sync_cursors SET status = 'idle', last_synced_at = NOW(), throttle_count = 0, throttle_after = null WHERE id = ${sc.id}`;
  } catch (e: any) {
    log.error(`sync ${userId}: ${e.message}`);
    await handleSyncError(sc, e);
  }
  return { ok: true };
}

async function runSync(token: string, userId: string, sc: any) {
  const foldersResp = await selfAction(token, "get_folders", {});
  const folders: Array<{ path: string; specialUse: string | null }> = (foldersResp?.folders ?? [])
    .filter((f: any) => !SKIP_SPECIAL_USE.has(f.specialUse));

  const cursor: Record<string, { uidValidity: number; highestUid: number }> = sc.cursor ? JSON.parse(sc.cursor) : {};
  const newCursor: Record<string, { uidValidity: number; highestUid: number }> = {};

  for (const folder of folders) {
    const prev = cursor[folder.path];
    const listResp = await selfAction(token, "list_emails", { folder: folder.path, sinceUid: prev?.highestUid });
    const { messageUids, uidValidity, highestUid } = listResp;

    if (prev && uidValidity !== prev.uidValidity) {
      log.warn(`uidValidity changed for folder ${folder.path}, full resync`);
      const fullResp = await selfAction(token, "list_emails", { folder: folder.path });
      await importUids(token, userId, folder.path, fullResp.messageUids ?? []);
      newCursor[folder.path] = { uidValidity: fullResp.uidValidity, highestUid: fullResp.highestUid };
      continue;
    }

    if (prev && highestUid <= (prev.highestUid ?? 0)) {
      newCursor[folder.path] = prev;
      continue;
    }

    await importUids(token, userId, folder.path, messageUids ?? []);
    newCursor[folder.path] = { uidValidity, highestUid };
  }

  await db`UPDATE imap_smtp.sync_cursors SET cursor = ${JSON.stringify(newCursor)} WHERE id = ${sc.id}`;
}

async function importUids(token: string, userId: string, folder: string, uids: number[]) {
  if (!uids.length) return;
  const externalIds = uids.map(uid => `${folder}:${uid}`);
  const existing = await db`SELECT external_id FROM imap_smtp.messages WHERE external_id = ANY(${externalIds})`;
  const existingSet = new Set(existing.map((r: any) => r.external_id));
  const missing = uids.filter(uid => !existingSet.has(`${folder}:${uid}`));
  if (!missing.length) return;

  for (let i = 0; i < missing.length; i += FETCH_CONCURRENCY) {
    const chunk = missing.slice(i, i + FETCH_CONCURRENCY);
    await Promise.allSettled(chunk.map(uid => persistSingleMessage(token, userId, uid, folder)));
  }
}

async function persistSingleMessage(token: string, userId: string, uid: number, folder: string) {
  let msg: any;
  try { msg = await selfAction(token, "get_email", { uid, folder }); }
  catch { return; }
  await persistParsedMessage(userId, msg);
}

async function persistParsedMessage(userId: string, msg: any) {
  const externalId = msg.externalId ?? `${msg.folder}:${msg.uid}`;
  const headerMsgId = msg.headerMessageId || `fallback-${userId}-${externalId}`;
  const threadId = msg.inReplyTo || msg.headerMessageId || externalId;

  const inserted = await db`
    INSERT INTO imap_smtp.messages (external_id, thread_external_id, header_message_id, user_id, folder, subject, body_text, body_html, snippet, internal_date, flags, in_reply_to, "references")
    VALUES (${externalId}, ${threadId}, ${headerMsgId}, ${userId}, ${msg.folder ?? ""}, ${msg.subject ?? ""}, ${msg.bodyText ?? ""}, ${msg.bodyHtml ?? ""}, ${(msg.bodyText ?? "").slice(0, 200)}, ${msg.date ? new Date(msg.date).toISOString() : null}, ${JSON.stringify(msg.flags ?? [])}, ${msg.inReplyTo ?? null}, ${JSON.stringify(msg.references ?? [])})
    ON CONFLICT (external_id) DO NOTHING
    RETURNING id
  `;
  if (!inserted.length) return;
  const msgDbId = inserted[0].id;

  if (threadId) {
    await db`
      INSERT INTO imap_smtp.threads (external_id, user_id, subject, last_message_at, message_count)
      VALUES (${threadId}, ${userId}, ${msg.subject ?? ""}, ${msg.date ? new Date(msg.date).toISOString() : null}, 1)
      ON CONFLICT (external_id) DO UPDATE SET
        last_message_at = GREATEST(imap_smtp.threads.last_message_at, EXCLUDED.last_message_at),
        message_count = imap_smtp.threads.message_count + 1
    `;
  }

  const parts: Array<{ message_id: string; address: string; name: string; role: string }> = [];
  if (msg.from) parts.push({ message_id: msgDbId, role: "from", address: msg.from.address ?? msg.from, name: msg.from.name ?? "" });
  for (const p of msg.to ?? []) parts.push({ message_id: msgDbId, role: "to", address: p.address ?? p, name: p.name ?? "" });
  for (const p of msg.cc ?? []) parts.push({ message_id: msgDbId, role: "cc", address: p.address ?? p, name: p.name ?? "" });
  if (parts.length) {
    await db`INSERT INTO imap_smtp.participants ${db(parts)} ON CONFLICT DO NOTHING`;
  }

  if (msg.attachments?.length) {
    const attRows = msg.attachments.map((a: any) => ({
      message_id: msgDbId, filename: a.filename ?? "", mime_type: a.mimeType ?? a.contentType ?? "",
      size: a.size ?? 0, content_id: a.contentId ?? null, is_inline: !!a.disposition?.includes("inline"),
    }));
    await db`INSERT INTO imap_smtp.attachments ${db(attRows)} ON CONFLICT DO NOTHING`;
  }
}

async function parseImapMessage(source: Buffer | Uint8Array, uid: number, folder: string, flags: string[]) {
  const parsed = await PostalMime.parse(source);
  return {
    uid,
    externalId: `${folder}:${uid}`,
    folder,
    headerMessageId: parsed.messageId ?? null,
    inReplyTo: parsed.headers?.find((h: any) => h.key === "in-reply-to")?.value ?? null,
    references: (parsed.headers?.find((h: any) => h.key === "references")?.value ?? "").split(/\s+/).filter(Boolean),
    from: parsed.from ?? null,
    to: parsed.to ?? [],
    cc: parsed.cc ?? [],
    subject: parsed.subject ?? "",
    date: parsed.date ? new Date(parsed.date).toISOString() : "",
    bodyText: parsed.text ?? "",
    bodyHtml: parsed.html ?? "",
    flags,
    attachments: (parsed.attachments ?? []).map((a: any) => ({
      filename: a.filename ?? "", mimeType: a.mimeType ?? "", size: a.content?.byteLength ?? 0,
      contentId: a.contentId ?? null, disposition: a.disposition ?? "",
    })),
  };
}

async function findSentFolder(client: ImapFlow): Promise<string | null> {
  const list = await client.list();
  const sent = list.find((f: any) => f.specialUse === "\\Sent");
  if (sent) return sent.path;
  const byName = list.find((f: any) => /^(sent|sent items|sent mail)$/i.test(f.name));
  return byName?.path ?? null;
}

async function selfAction(token: string, action: string, input: any): Promise<any> {
  const res = await fetch(`${ctx.runtimeUrl}/api/v1/integrations/imap_smtp/actions/${action}`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}` },
    body: JSON.stringify(input),
  });
  if (!res.ok) throw new Error(`imap_smtp/${action} -> ${res.status}`);
  const result = await res.json();
  if (result?.ok === false && result?.error) {
    const e: any = new Error(result.error.message ?? action);
    e.code = result.error.code;
    throw e;
  }
  return result?.ok === true && "data" in result ? result.data : result;
}

async function dispatchSyncJob(userId: string, token: string) {
  await db`UPDATE imap_smtp.sync_cursors SET status = 'syncing' WHERE user_id = ${userId}`;
  await runtimeFetch("POST", "/api/v1/apps/imap_smtp/jobs", token, { payload: { type: "sync", user_id: userId } })
    .catch(e => log.warn(`job dispatch: ${e.message}`));
}

async function createSyncCron(userId: string, token: string): Promise<string | null> {
  try {
    const res = await runtimeFetch("POST", "/api/v1/apps/imap_smtp/crons", token, {
      name: `sync_imap_${userId}`, schedule: "*/5 * * * *",
      payload: { type: "sync", user_id: userId }, overlapPolicy: "skip",
    });
    return res?.id ?? null;
  } catch (e: any) { log.warn(`cron create: ${e.message}`); return null; }
}

async function deleteSyncCron(cronId: string, token: string) {
  await runtimeFetch("DELETE", `/api/v1/apps/imap_smtp/crons/${cronId}`, token).catch(e => log.warn(`cron delete: ${e.message}`));
}

async function runtimeFetch(method: string, path: string, token: string, body?: any): Promise<any> {
  const res = await fetch(`${ctx.runtimeUrl}${path}`, {
    method, headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}` },
    ...(body ? { body: JSON.stringify(body) } : {}),
  });
  if (!res.ok) throw new Error(`${method} ${path} -> ${res.status}`);
  return res.json().catch(() => null);
}

async function handleSyncError(sc: any, err: any) {
  const count = (sc.throttle_count ?? 0) + 1;
  if (count >= MAX_THROTTLE) {
    await db`UPDATE imap_smtp.sync_cursors SET status = 'failed_permanent', throttle_count = ${count} WHERE id = ${sc.id}`;
    return;
  }
  const wait = 60_000 * Math.pow(2, Math.min(count - 1, 5));
  const after = new Date(Date.now() + wait).toISOString();
  await db`UPDATE imap_smtp.sync_cursors SET status = 'failed_temporary', throttle_count = ${count}, throttle_after = ${after} WHERE id = ${sc.id}`;
}

async function listEmails(creds: Creds, input: any, _userId: string, _token: string) {
  const { folder = "INBOX", sinceUid } = input ?? {};
  return withImap(creds, async (client) => {
    const lock = await client.getMailboxLock(folder);
    try {
      const uidValidity = Number(client.mailbox?.uidValidity ?? 0);
      const uidNext = Number(client.mailbox?.uidNext ?? 1);
      const highestUid = Math.max(0, uidNext - 1);
      if (highestUid === 0) return { ok: true, data: { messageUids: [], uidValidity, highestUid } };
      const startUid = sinceUid ? sinceUid + 1 : 1;
      if (startUid > highestUid) return { ok: true, data: { messageUids: [], uidValidity, highestUid } };
      const uids = await client.search({ uid: `${startUid}:${highestUid}` }, { uid: true });
      return { ok: true, data: { messageUids: Array.isArray(uids) ? uids : [], uidValidity, highestUid } };
    } finally { lock.release(); }
  });
}
actions["list_emails"] = listEmails;
