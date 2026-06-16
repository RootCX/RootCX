/// <reference path="../rootcx-worker.d.ts" />
import { ImapFlow } from "imapflow";
import { createTransport } from "nodemailer";
import MailComposer from "nodemailer/lib/mail-composer";
import PostalMime from "postal-mime";

const MAX_THROTTLE = 5;
const FETCH_CONCURRENCY = 10;
const SKIP_SPECIAL_USE = new Set(["\\Junk", "\\Trash", "\\Drafts"]);

interface Creds { imapHost: string; imapPort: number; smtpHost: string; username: string; password: string }

serve({
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
    async __integration(params, _caller, ctx) {
      const { action, input, userCredentials, userId } = params;
      if (!userCredentials?.imapHost || !userCredentials?.username) {
        return { ok: false, error: { code: "INSUFFICIENT_PERMISSIONS", message: "not connected" } };
      }
      const handler = actions[action];
      if (!handler) return { ok: false, error: { code: "MISCONFIGURED", message: `unknown action: ${action}` } };
      return handler(userCredentials as Creds, input, userId, ctx);
    },
  },
  onJob: handleJob,
});

// Re-enter one of our own actions (with the user's resolved credentials) and
// surface its Result: return `data`, or throw a coded Error so callers can
// branch on `err.code` (e.g. SYNC_CURSOR_ERROR).
async function runAction(ctx: RootCxCtx, action: string, input: Record<string, unknown> = {}): Promise<any> {
  const r = await ctx.action(action, input);
  if (r?.ok === false) {
    throw Object.assign(new Error(r.error?.message ?? action), { code: r.error?.code, retryAfter: r.error?.retryAfter });
  }
  return r?.ok === true && "data" in r ? r.data : r;
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

async function sendEmail(creds: Creds, input: any, userId: string, ctx: RootCxCtx) {
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

  try {
    const parsed = await runAction(ctx, "get_email", { uid: 0, folder: "SENT", messageId });
    if (parsed) await persistParsedMessage(ctx, userId, parsed);
  } catch { /* will be picked up on next sync */ }

  return { ok: true, data: { messageId, ok: true } };
}

async function getEmail(creds: Creds, input: any, _userId: string, _ctx: RootCxCtx) {
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

async function getFolders(creds: Creds, _input: any, _userId: string, _ctx: RootCxCtx) {
  const folders = await withImap(creds, async (client) => {
    const list = await client.list();
    return list
      .filter((f: any) => !f.flags?.has("\\Noselect"))
      .map((f: any) => ({ path: f.path, name: f.name, specialUse: f.specialUse ?? null, flags: [...(f.flags ?? [])] }));
  });
  return { ok: true, data: { folders } };
}

async function syncConnect(creds: Creds, _input: any, userId: string, ctx: RootCxCtx) {
  const handle = creds.username.toLowerCase();
  const existing = await ctx.sql(
    `SELECT id FROM imap_smtp.sync_cursors WHERE user_id = $1`, [userId]
  );
  let cursorId: string;
  if (existing.rows.length) {
    cursorId = existing.rows[0][0] as string;
    await ctx.sql(
      `UPDATE imap_smtp.sync_cursors SET handle = $1, enabled = true, status = 'idle', throttle_count = 0, throttle_after = null WHERE id = $2`,
      [handle, cursorId]
    );
  } else {
    const ins = await ctx.sql(
      `INSERT INTO imap_smtp.sync_cursors (user_id, handle, status, enabled, throttle_count) VALUES ($1, $2, 'idle', true, 0) RETURNING id`,
      [userId, handle]
    );
    cursorId = ins.rows[0][0] as string;
  }
  await syncUserNow(ctx, userId);
  return { ok: true, data: { cursor_id: cursorId, handle } };
}

async function syncDisconnect(_creds: Creds, _input: any, userId: string, ctx: RootCxCtx) {
  await ctx.sql(`UPDATE imap_smtp.sync_cursors SET enabled = false WHERE user_id = $1`, [userId]);
  return { ok: true, data: {} };
}

async function syncNow(_creds: Creds, _input: any, userId: string, ctx: RootCxCtx) {
  const cursor = await ctx.sql(
    `SELECT id, status, throttle_after FROM imap_smtp.sync_cursors WHERE user_id = $1 AND enabled = true`, [userId]
  );
  if (!cursor.rows.length) return { ok: false, error: { code: "MISCONFIGURED", message: "no active sync" } };
  const [, status, throttle_after] = cursor.rows[0] as any[];
  if (status === "syncing") return { ok: true, data: { triggered: false } };
  if (throttle_after && new Date(throttle_after).getTime() > Date.now()) return { ok: true, data: { triggered: false } };
  await syncUserNow(ctx, userId);
  return { ok: true, data: { triggered: true } };
}

const actions: Record<string, (creds: Creds, input: any, userId: string, ctx: RootCxCtx) => Promise<any>> = {
  send_email: sendEmail, get_email: getEmail, get_folders: getFolders,
  sync_connect: syncConnect, sync_disconnect: syncDisconnect, sync_now: syncNow,
};

async function handleJob(payload: any, _caller: any, ctx: RootCxCtx) {
  if (payload?.type === "sync_all") {
    await ctx.selfAction("syncConnectedUsers", { actionName: "sync_now" });
    return { ok: true };
  }
  if (payload?.type === "sync") {
    await syncUserNow(ctx, payload.user_id);
    return { ok: true };
  }
  return { skipped: true };
}

async function syncUserNow(ctx: RootCxCtx, userId: string) {
  const cursors = await ctx.sql(
    `SELECT id, cursor, throttle_count FROM imap_smtp.sync_cursors WHERE user_id = $1 AND enabled = true`, [userId]
  );
  if (!cursors.rows.length) return;
  const [id, cursor, throttle_count] = cursors.rows[0] as any[];
  const sc = { id, cursor, throttle_count };
  try {
    await ctx.sql(`UPDATE imap_smtp.sync_cursors SET status = 'syncing' WHERE id = $1`, [sc.id]);
    await runSync(ctx, userId, sc);
    await ctx.sql(
      `UPDATE imap_smtp.sync_cursors SET status = 'idle', last_synced_at = NOW(), throttle_count = 0, throttle_after = null WHERE id = $1`, [sc.id]
    );
  } catch (e: any) {
    log.error(`sync ${userId}: ${e.message}`);
    await handleSyncError(ctx, sc, e);
  }
}

async function runSync(ctx: RootCxCtx, userId: string, sc: any) {
  const foldersResp = await runAction(ctx, "get_folders", {});
  const folders: Array<{ path: string; specialUse: string | null }> = (foldersResp?.folders ?? [])
    .filter((f: any) => !SKIP_SPECIAL_USE.has(f.specialUse));

  const cursor: Record<string, { uidValidity: number; highestUid: number }> = sc.cursor ? JSON.parse(sc.cursor) : {};
  const newCursor: Record<string, { uidValidity: number; highestUid: number }> = {};

  for (const folder of folders) {
    const prev = cursor[folder.path];
    const listResp = await runAction(ctx, "list_emails", { folder: folder.path, sinceUid: prev?.highestUid });
    const { messageUids, uidValidity, highestUid } = listResp;

    if (prev && uidValidity !== prev.uidValidity) {
      log.warn(`uidValidity changed for folder ${folder.path}, full resync`);
      const fullResp = await runAction(ctx, "list_emails", { folder: folder.path });
      await importUids(ctx, userId, folder.path, fullResp.messageUids ?? []);
      newCursor[folder.path] = { uidValidity: fullResp.uidValidity, highestUid: fullResp.highestUid };
      continue;
    }

    if (prev && highestUid <= (prev.highestUid ?? 0)) {
      newCursor[folder.path] = prev;
      continue;
    }

    await importUids(ctx, userId, folder.path, messageUids ?? []);
    newCursor[folder.path] = { uidValidity, highestUid };
  }

  await ctx.sql(`UPDATE imap_smtp.sync_cursors SET cursor = $1 WHERE id = $2`, [JSON.stringify(newCursor), sc.id]);
}

async function importUids(ctx: RootCxCtx, userId: string, folder: string, uids: number[]) {
  if (!uids.length) return;
  const externalIds = uids.map(uid => `${folder}:${uid}`);
  const existing = await ctx.sql(
    `SELECT external_id FROM imap_smtp.messages WHERE external_id = ANY($1)`, [externalIds]
  );
  const existingSet = new Set(existing.rows.map((r: any) => r[0]));
  const missing = uids.filter(uid => !existingSet.has(`${folder}:${uid}`));
  if (!missing.length) return;

  for (let i = 0; i < missing.length; i += FETCH_CONCURRENCY) {
    const chunk = missing.slice(i, i + FETCH_CONCURRENCY);
    await Promise.allSettled(chunk.map(uid => persistSingleMessage(ctx, userId, uid, folder)));
  }
}

async function persistSingleMessage(ctx: RootCxCtx, userId: string, uid: number, folder: string) {
  let msg: any;
  try { msg = await runAction(ctx, "get_email", { uid, folder }); }
  catch { return; }
  await persistParsedMessage(ctx, userId, msg);
}

async function persistParsedMessage(ctx: RootCxCtx, userId: string, msg: any) {
  const externalId = msg.externalId ?? `${msg.folder}:${msg.uid}`;
  const headerMsgId = msg.headerMessageId || `fallback-${userId}-${externalId}`;
  const threadId = msg.inReplyTo || msg.headerMessageId || externalId;
  const internalDate = msg.date ? new Date(msg.date).toISOString() : null;

  const inserted = await ctx.sql(
    `INSERT INTO imap_smtp.messages (external_id, thread_external_id, header_message_id, user_id, folder, subject, body_text, body_html, snippet, internal_date, flags, in_reply_to, "references")
     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
     ON CONFLICT (external_id) DO NOTHING
     RETURNING id`,
    [externalId, threadId, headerMsgId, userId, msg.folder ?? "", msg.subject ?? "", msg.bodyText ?? "", msg.bodyHtml ?? "", (msg.bodyText ?? "").slice(0, 200), internalDate, JSON.stringify(msg.flags ?? []), msg.inReplyTo ?? null, JSON.stringify(msg.references ?? [])]
  );
  if (!inserted.rows.length) return;
  const msgDbId = inserted.rows[0][0];

  if (threadId) {
    await ctx.sql(
      `INSERT INTO imap_smtp.threads (external_id, user_id, subject, last_message_at, message_count)
       VALUES ($1, $2, $3, $4, 1)
       ON CONFLICT (external_id) DO UPDATE SET
         last_message_at = GREATEST(imap_smtp.threads.last_message_at, EXCLUDED.last_message_at),
         message_count = imap_smtp.threads.message_count + 1`,
      [threadId, userId, msg.subject ?? "", internalDate]
    );
  }

  const parts: Array<[string, string, string, string]> = [];
  if (msg.from) parts.push([msgDbId, "from", msg.from.address ?? msg.from, msg.from.name ?? ""]);
  for (const p of msg.to ?? []) parts.push([msgDbId, "to", p.address ?? p, p.name ?? ""]);
  for (const p of msg.cc ?? []) parts.push([msgDbId, "cc", p.address ?? p, p.name ?? ""]);
  if (parts.length) {
    const pCols = 4;
    const pValues = parts.map((_, i) => `($${i*pCols+1}, $${i*pCols+2}, $${i*pCols+3}, $${i*pCols+4})`).join(", ");
    const pParams = parts.flatMap(([mid, role, address, name]) => [mid, role, address, name]);
    await ctx.sql(
      `INSERT INTO imap_smtp.participants (message_id, role, address, name) VALUES ${pValues} ON CONFLICT DO NOTHING`,
      pParams
    );
  }

  const attachments = msg.attachments ?? [];
  if (attachments.length) {
    const atCols = 6;
    const atValues = attachments.map((_: any, i: number) => `($${i*atCols+1}, $${i*atCols+2}, $${i*atCols+3}, $${i*atCols+4}, $${i*atCols+5}, $${i*atCols+6})`).join(", ");
    const atParams = attachments.flatMap((a: any) => [msgDbId, a.filename ?? "", a.mimeType ?? a.contentType ?? "", a.size ?? 0, a.contentId ?? null, !!a.disposition?.includes("inline")]);
    await ctx.sql(
      `INSERT INTO imap_smtp.attachments (message_id, filename, mime_type, size, content_id, is_inline) VALUES ${atValues} ON CONFLICT DO NOTHING`,
      atParams
    );
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


async function handleSyncError(ctx: RootCxCtx, sc: any, err: any) {
  const count = (sc.throttle_count ?? 0) + 1;
  if (count >= MAX_THROTTLE) {
    await ctx.sql(`UPDATE imap_smtp.sync_cursors SET status = 'failed_permanent', throttle_count = $1 WHERE id = $2`, [count, sc.id]);
    return;
  }
  const wait = 60_000 * Math.pow(2, Math.min(count - 1, 5));
  const after = new Date(Date.now() + wait).toISOString();
  await ctx.sql(
    `UPDATE imap_smtp.sync_cursors SET status = 'failed_temporary', throttle_count = $1, throttle_after = $2 WHERE id = $3`,
    [count, after, sc.id]
  );
}

async function listEmails(creds: Creds, input: any, _userId: string, _ctx: RootCxCtx) {
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
