/// <reference path="../rootcx-worker.d.ts" />
import {
  type Config, type UserCreds,
  oauth2ClientFor, buildAuthUrl, exchangeCodeForRefreshToken, evictClient,
} from "./lib/oauth";
import { type Result, ok, fail, classifyHttp, withRetry } from "./lib/errors";
import { parseMessage } from "./lib/parse-message";
import { composeMessage, type SendInput } from "./lib/mail-composer";
import { cacheAliases, getCachedAliases } from "./lib/aliases";

const GMAIL_API = "https://www.googleapis.com/gmail/v1/users/me";
const EXCLUDE_QUERY = "-in:spam -in:trash -in:drafts -category:promotions -category:social";
const MAX_THROTTLE = 5;
const FETCH_CONCURRENCY = 15;

serve({
  rpc: {
    __auth_start: authStart,
    __auth_callback: authCallback,
    async __integration(params, _caller, ctx) {
      const { action, input, config, userCredentials, userId } = params;
      if (!userCredentials?.refreshToken) return fail({ code: "INSUFFICIENT_PERMISSIONS", message: "user not connected" });
      const handler = actions[action];
      if (!handler) return fail({ code: "MISCONFIGURED", message: `unknown action: ${action}` });
      return handler(config, userCredentials, input, userId, ctx);
    },
    async __webhook(params) {
      const data = params?.body?.message?.data;
      if (!data) return { skipped: true, reason: "no push data" };
      let decoded: any;
      try { decoded = JSON.parse(Buffer.from(data, "base64url").toString("utf-8")); } catch { return { skipped: true, reason: "invalid envelope" }; }
      if (!decoded.historyId || !decoded.emailAddress) return { skipped: true, reason: "missing fields" };
      return { event: "push_notification", emailAddress: String(decoded.emailAddress).toLowerCase(), historyId: String(decoded.historyId) };
    },
  },
  onJob: handleJob,
});

async function accessTokenFor(config: Config, creds: UserCreds, userId: string): Promise<string> {
  const access = await oauth2ClientFor(config, creds, userId).getAccessToken();
  if (!access.token) throw new Error("no access token from refresh");
  return access.token;
}

async function gmailApi(config: Config, creds: UserCreds, path: string, init: RequestInit | undefined, userId: string): Promise<Result<any>> {
  let token: string;
  try { token = await accessTokenFor(config, creds, userId); }
  catch (e: any) { evictClient(userId); return fail({ code: "INSUFFICIENT_PERMISSIONS", message: e?.message ?? "auth failed" }); }
  let res: Response;
  try { res = await fetch(`${GMAIL_API}${path}`, { ...init, headers: { Authorization: `Bearer ${token}`, ...init?.headers } }); }
  catch (e: any) { return fail({ code: "TEMPORARY_ERROR", message: e?.message ?? "network error" }); }
  if (!res.ok) {
    const body = await res.text().catch(() => "");
    const err = classifyHttp(res.status, body, path);
    if (err.code === "INSUFFICIENT_PERMISSIONS") evictClient(userId);
    return fail(err);
  }
  return ok(await res.json());
}

const callGmail = (config: Config, creds: UserCreds, path: string, init: RequestInit | undefined, userId: string) =>
  withRetry(() => gmailApi(config, creds, path, init, userId));

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

async function authStart(params: any) {
  const { config, callbackUrl, state } = params;
  return { type: "redirect", url: buildAuthUrl(config, callbackUrl, state) };
}

async function authCallback(params: any) {
  const { config, query } = params;
  const refreshToken = await exchangeCodeForRefreshToken(config, query.code, query.redirect_uri ?? params.callbackUrl ?? "");
  return { credentials: { refreshToken } };
}

async function fetchAndCacheAliases(config: Config, creds: UserCreds, userId: string): Promise<Result<{ primary: string; aliases: string[] }>> {
  const cached = getCachedAliases(userId);
  if (cached) return ok({ primary: cached.primary, aliases: [...cached.aliases] });
  const [profileR, sendAsR] = await Promise.all([
    callGmail(config, creds, "/profile", undefined, userId),
    callGmail(config, creds, "/settings/sendAs", undefined, userId),
  ]);
  if (!profileR.ok) return profileR;
  if (!sendAsR.ok) return sendAsR;
  const primary = (profileR.data.emailAddress ?? "").toLowerCase();
  const aliases = (sendAsR.data.sendAs ?? []).map((e: any) => (e.sendAsEmail ?? "").toLowerCase()).filter(Boolean);
  cacheAliases(userId, primary, aliases);
  return ok({ primary, aliases });
}

async function composeAndSend(
  config: Config, creds: UserCreds, input: SendInput, userId: string, asDraft: boolean, ctx: RootCxCtx,
): Promise<Result<any>> {
  if (!input.to && !input.bcc) return fail({ code: "MISCONFIGURED", message: "at least one recipient required" });
  const aliases = await fetchAndCacheAliases(config, creds, userId);
  if (!aliases.ok) return aliases;

  let composed;
  try { composed = await composeMessage(input, input.from ?? aliases.data.primary); }
  catch (e: any) { return fail({ code: e?._tooLarge ? "MISCONFIGURED" : "UNKNOWN", message: e?.message ?? "compose failed" }); }

  const threadPart = input.threadId ? { threadId: input.threadId } : {};
  const path = asDraft ? "/drafts" : "/messages/send";
  const body = asDraft
    ? { message: { raw: composed.rawBase64Url, ...threadPart } }
    : { raw: composed.rawBase64Url, ...threadPart };

  const r = await callGmail(config, creds, path, {
    method: "POST", headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  }, userId);
  if (!r.ok) return r;

  const msgId = asDraft ? r.data.message?.id : r.data.id;
  if (msgId) await persistSingleMessage(ctx, userId, msgId);

  return asDraft
    ? ok({ draftId: r.data.id, messageId: msgId, threadId: r.data.message?.threadId, headerMessageId: composed.headerMessageId })
    : ok({ messageId: r.data.id, threadId: r.data.threadId, headerMessageId: composed.headerMessageId });
}

const sendEmail = (c: Config, u: UserCreds, i: any, uid: string, ctx: RootCxCtx) => composeAndSend(c, u, i, uid, false, ctx);
const createDraft = (c: Config, u: UserCreds, i: any, uid: string, ctx: RootCxCtx) => composeAndSend(c, u, i, uid, true, ctx);

async function getAttachment(config: Config, creds: UserCreds, input: any, userId: string, _ctx: RootCxCtx): Promise<Result<any>> {
  if (!input.messageId || !input.attachmentId) return fail({ code: "MISCONFIGURED", message: "messageId and attachmentId required" });
  return callGmail(config, creds, `/messages/${input.messageId}/attachments/${input.attachmentId}`, undefined, userId);
}

async function syncConnect(config: Config, creds: UserCreds, _input: any, userId: string, ctx: RootCxCtx): Promise<Result<any>> {
  const aliases = await fetchAndCacheAliases(config, creds, userId);
  if (!aliases.ok) return aliases;
  const { primary: handle, aliases: aliasList } = aliases.data;

  const existing = await ctx.sql(
    `SELECT id, cron_id FROM gmail.sync_cursors WHERE user_id = $1`, [userId]
  );
  let cursorId: string;
  if (existing.rows.length) {
    cursorId = existing.rows[0][0];
    await ctx.sql(
      `UPDATE gmail.sync_cursors SET handle = $1, handle_aliases = $2, enabled = true, status = 'idle', throttle_count = 0, throttle_after = null WHERE id = $3`,
      [handle, JSON.stringify(aliasList), cursorId]
    );
  } else {
    const ins = await ctx.sql(
      `INSERT INTO gmail.sync_cursors (user_id, handle, handle_aliases, status, sync_stage, enabled, throttle_count) VALUES ($1, $2, $3, 'idle', 'list_fetch', true, 0) RETURNING id`,
      [userId, handle, JSON.stringify(aliasList)]
    );
    cursorId = ins.rows[0][0];
  }

  await syncUserNow(userId, ctx);
  return ok({ cursor_id: cursorId, handle });
}

async function syncDisconnect(_config: Config, _creds: UserCreds, _input: any, userId: string, ctx: RootCxCtx): Promise<Result<any>> {
  await ctx.sql(`UPDATE gmail.sync_cursors SET enabled = false WHERE user_id = $1`, [userId]);
  return ok({});
}

async function syncNow(_config: Config, _creds: UserCreds, _input: any, userId: string, ctx: RootCxCtx): Promise<Result<any>> {
  const cursor = await ctx.sql(
    `SELECT id, status, throttle_after FROM gmail.sync_cursors WHERE user_id = $1 AND enabled = true`, [userId]
  );
  if (!cursor.rows.length) return fail({ code: "MISCONFIGURED", message: "no active sync" });
  const [, status, throttle_after] = cursor.rows[0];
  if (status === "syncing") return ok({ triggered: false });
  if (throttle_after && new Date(throttle_after).getTime() > Date.now()) return ok({ triggered: false });
  await syncUserNow(userId, ctx);
  return ok({ triggered: true });
}

async function syncUserNow(userId: string, ctx: RootCxCtx) {
  await ctx.sql(`UPDATE gmail.sync_cursors SET status = 'syncing' WHERE user_id = $1`, [userId]);
  const cursors = await ctx.sql(
    `SELECT id, cursor, sync_stage, throttle_count FROM gmail.sync_cursors WHERE user_id = $1 AND enabled = true`, [userId]
  );
  if (!cursors.rows.length) return;
  const [id, cursor, , throttle_count] = cursors.rows[0];
  const sc = { id, cursor, throttle_count };
  try {
    if (!sc.cursor) await fullSync(ctx, userId, sc);
    else await incrementalSync(ctx, userId, sc);
    await ctx.sql(
      `UPDATE gmail.sync_cursors SET status = 'idle', last_synced_at = NOW(), throttle_count = 0, throttle_after = null WHERE id = $1`, [sc.id]
    );
  } catch (e: any) {
    log.error(`sync ${userId}: ${e.message}`);
    await handleSyncError(ctx, sc, e);
  }
}

async function getProfile(config: Config, creds: UserCreds, _input: any, userId: string, _ctx: RootCxCtx): Promise<Result<any>> {
  const r = await callGmail(config, creds, "/profile", undefined, userId);
  if (!r.ok) return r;
  return ok({ emailAddress: r.data.emailAddress, historyId: r.data.historyId });
}

async function listSendAs(config: Config, creds: UserCreds, _input: any, userId: string, _ctx: RootCxCtx): Promise<Result<any>> {
  const r = await callGmail(config, creds, "/settings/sendAs", undefined, userId);
  if (!r.ok) return r;
  return ok({ sendAs: (r.data.sendAs ?? []).map((e: any) => ({ sendAsEmail: e.sendAsEmail, displayName: e.displayName ?? "", isDefault: !!e.isDefault, isPrimary: !!e.isPrimary })) });
}

async function watchAction(config: Config, creds: UserCreds, input: any, userId: string, _ctx: RootCxCtx): Promise<Result<any>> {
  const topicName = input?.topicName ?? config.pubsubTopicName;
  if (!topicName) return fail({ code: "MISCONFIGURED", message: "topicName required" });
  const r = await callGmail(config, creds, "/watch", {
    method: "POST", headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ topicName, ...(input?.labelIds ? { labelIds: input.labelIds } : {}) }),
  }, userId);
  if (!r.ok) return r;
  return ok({ historyId: r.data.historyId, expiration: r.data.expiration ? Number(r.data.expiration) : null });
}

async function stopWatch(config: Config, creds: UserCreds, _input: any, userId: string, _ctx: RootCxCtx): Promise<Result<any>> {
  const r = await callGmail(config, creds, "/stop", { method: "POST", headers: { "Content-Type": "application/json" }, body: "{}" }, userId);
  if (!r.ok) return r;
  return ok({});
}

async function listMessages(config: Config, creds: UserCreds, input: any, userId: string, _ctx: RootCxCtx): Promise<Result<any>> {
  const { query, maxResults = 100, pageToken, labelIds } = input ?? {};
  const params = new URLSearchParams({ maxResults: String(Math.min(Math.max(1, maxResults), 500)) });
  if (query) params.set("q", String(query).slice(0, 1024));
  if (pageToken) params.set("pageToken", pageToken);
  if (labelIds?.length) for (const l of labelIds) params.append("labelIds", l);
  const r = await callGmail(config, creds, `/messages?${params}`, undefined, userId);
  if (!r.ok) return r;
  return ok({
    messages: (r.data.messages ?? []).map((m: any) => ({ id: m.id, threadId: m.threadId })),
    nextPageToken: r.data.nextPageToken ?? null,
    resultSizeEstimate: r.data.resultSizeEstimate ?? 0,
  });
}

async function getEmail(config: Config, creds: UserCreds, input: any, userId: string, _ctx: RootCxCtx): Promise<Result<any>> {
  if (!input?.messageId) return fail({ code: "MISCONFIGURED", message: "messageId required" });
  const r = await callGmail(config, creds, `/messages/${input.messageId}?format=full`, undefined, userId);
  if (!r.ok) return r;
  return ok(parseMessage(r.data));
}

async function historyList(config: Config, creds: UserCreds, input: any, userId: string, _ctx: RootCxCtx): Promise<Result<any>> {
  const { startHistoryId, maxResults = 500, pageToken } = input ?? {};
  if (!startHistoryId) return fail({ code: "MISCONFIGURED", message: "startHistoryId required" });
  const params = new URLSearchParams({ startHistoryId: String(startHistoryId), maxResults: String(Math.min(maxResults, 500)) });
  if (pageToken) params.set("pageToken", pageToken);
  params.append("historyTypes", "messageAdded");
  params.append("historyTypes", "messageDeleted");
  const r = await callGmail(config, creds, `/history?${params}`, undefined, userId);
  if (!r.ok) return r;
  const messagesAdded: string[] = [];
  const messagesDeleted: string[] = [];
  for (const entry of r.data.history ?? []) {
    for (const m of entry.messagesAdded ?? []) if (m.message?.id) messagesAdded.push(m.message.id);
    for (const m of entry.messagesDeleted ?? []) if (m.message?.id) messagesDeleted.push(m.message.id);
  }
  const deletedSet = new Set(messagesDeleted);
  return ok({
    messagesAdded: [...new Set(messagesAdded)].filter(id => !deletedSet.has(id)),
    messagesDeleted: [...deletedSet],
    historyId: r.data.historyId ?? null,
    nextPageToken: r.data.nextPageToken ?? null,
  });
}

const actions: Record<string, (c: Config, u: UserCreds, i: any, uid: string, ctx: RootCxCtx) => Promise<Result<any>>> = {
  send_email: sendEmail, create_draft: createDraft, get_attachment: getAttachment,
  sync_connect: syncConnect, sync_disconnect: syncDisconnect, sync_now: syncNow,
  get_profile: getProfile, list_send_as: listSendAs, watch: watchAction, stop_watch: stopWatch,
  list_messages: listMessages, get_email: getEmail, history_list: historyList,
};

async function handleJob(payload: any, _caller: any, ctx: RootCxCtx) {
  if (payload?.type === "sync_all") {
    await ctx.selfAction("syncConnectedUsers", { actionName: "sync_now" });
    return { ok: true };
  }
  if (payload?.type === "sync") {
    await syncUserNow(payload.user_id, ctx);
    return { ok: true };
  }
  return { skipped: true };
}

async function fullSync(ctx: RootCxCtx, userId: string, sc: any) {
  const profile = await runAction(ctx, "get_profile");
  const snapshotCursor = profile.historyId;

  let pageToken: string | undefined;
  do {
    const r = await runAction(ctx, "list_messages", { query: EXCLUDE_QUERY, maxResults: 500, pageToken });
    const ids: string[] = (r.messages ?? []).map((m: any) => m.id);
    await importBatch(ctx, userId, ids);
    pageToken = r.nextPageToken ?? undefined;
  } while (pageToken);

  await ctx.sql(
    `UPDATE gmail.sync_cursors SET cursor = $1, sync_stage = 'list_fetch' WHERE id = $2`,
    [snapshotCursor, sc.id]
  );
}

async function incrementalSync(ctx: RootCxCtx, userId: string, sc: any) {
  let pageToken: string | undefined;
  let latestHistoryId: string | null = null;
  do {
    let r: any;
    try {
      r = await runAction(ctx, "history_list", { startHistoryId: sc.cursor, maxResults: 500, pageToken });
    } catch (e: any) {
      if (e.code === "SYNC_CURSOR_ERROR") {
        await ctx.sql(`UPDATE gmail.sync_cursors SET cursor = null WHERE id = $1`, [sc.id]);
        return fullSync(ctx, userId, { ...sc, cursor: null });
      }
      throw e;
    }
    const added: string[] = r.messagesAdded ?? [];
    const deleted: string[] = r.messagesDeleted ?? [];
    if (deleted.length) {
      await ctx.sql(
        `DELETE FROM gmail.messages WHERE external_id = ANY($1) AND user_id = $2`, [deleted, userId]
      );
    }
    if (added.length) await importBatch(ctx, userId, added);
    latestHistoryId = r.historyId ?? latestHistoryId;
    pageToken = r.nextPageToken ?? undefined;
  } while (pageToken);

  if (latestHistoryId) {
    await ctx.sql(`UPDATE gmail.sync_cursors SET cursor = $1 WHERE id = $2`, [latestHistoryId, sc.id]);
  }
}

async function importBatch(ctx: RootCxCtx, userId: string, externalIds: string[]) {
  if (!externalIds.length) return;
  const existing = await ctx.sql(
    `SELECT external_id FROM gmail.messages WHERE external_id = ANY($1)`, [externalIds]
  );
  const existingSet = new Set(existing.rows.map((r: any) => r[0]));
  const missing = externalIds.filter(id => !existingSet.has(id));
  if (!missing.length) return;

  for (let i = 0; i < missing.length; i += FETCH_CONCURRENCY) {
    const chunk = missing.slice(i, i + FETCH_CONCURRENCY);
    await Promise.allSettled(chunk.map(id => persistSingleMessage(ctx, userId, id)));
  }
}

async function persistSingleMessage(ctx: RootCxCtx, userId: string, externalId: string) {
  let msg: any;
  try { msg = await runAction(ctx, "get_email", { messageId: externalId }); }
  catch { return; }

  const headerMsgId = msg.headerMessageId || `fallback-${userId}-${externalId}`;
  const internalDate = msg.internalDate ? new Date(msg.internalDate).toISOString() : null;

  const inserted = await ctx.sql(
    `INSERT INTO gmail.messages (external_id, thread_external_id, header_message_id, user_id, subject, body_text, body_html, snippet, internal_date, label_ids, in_reply_to, "references")
     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
     ON CONFLICT (external_id) DO NOTHING
     RETURNING id`,
    [externalId, msg.threadId, headerMsgId, userId, msg.subject, msg.bodyText, msg.bodyHtml, msg.snippet, internalDate, JSON.stringify(msg.labelIds), msg.inReplyTo, JSON.stringify(msg.references)]
  );
  if (!inserted.rows.length) return;
  const msgDbId = inserted.rows[0][0];

  if (msg.threadId) {
    await ctx.sql(
      `INSERT INTO gmail.threads (external_id, user_id, subject, last_message_at, message_count)
       VALUES ($1, $2, $3, $4, 1)
       ON CONFLICT (external_id) DO UPDATE SET
         last_message_at = GREATEST(gmail.threads.last_message_at, EXCLUDED.last_message_at),
         message_count = gmail.threads.message_count + 1`,
      [msg.threadId, userId, msg.subject, internalDate]
    );
  }

  const parts: Array<[string, string, string, string]> = [];
  if (msg.from) parts.push([msgDbId, "from", msg.from.address ?? msg.from, msg.from.name ?? ""]);
  for (const p of msg.to ?? []) parts.push([msgDbId, "to", p.address ?? p, p.name ?? ""]);
  for (const p of msg.cc ?? []) parts.push([msgDbId, "cc", p.address ?? p, p.name ?? ""]);
  for (const p of msg.bcc ?? []) parts.push([msgDbId, "bcc", p.address ?? p, p.name ?? ""]);
  if (parts.length) {
    const pCols = 4;
    const pValues = parts.map((_, i) => `($${i*pCols+1}, $${i*pCols+2}, $${i*pCols+3}, $${i*pCols+4})`).join(", ");
    const pParams = parts.flatMap(([mid, role, address, name]) => [mid, role, address, name]);
    await ctx.sql(
      `INSERT INTO gmail.participants (message_id, role, address, name) VALUES ${pValues} ON CONFLICT DO NOTHING`,
      pParams
    );
  }

  const attachments = msg.attachments ?? [];
  if (attachments.length) {
    const atCols = 7;
    const atValues = attachments.map((_: any, i: number) => `($${i*atCols+1}, $${i*atCols+2}, $${i*atCols+3}, $${i*atCols+4}, $${i*atCols+5}, $${i*atCols+6}, $${i*atCols+7})`).join(", ");
    const atParams = attachments.flatMap((a: any) => [msgDbId, a.id, a.filename, a.mimeType, a.size, a.contentId, a.isInline]);
    await ctx.sql(
      `INSERT INTO gmail.attachments (message_id, external_id, filename, mime_type, size, content_id, is_inline) VALUES ${atValues} ON CONFLICT DO NOTHING`,
      atParams
    );
  }
}

async function handleSyncError(ctx: RootCxCtx, sc: any, err: any) {
  if (err.code === "INSUFFICIENT_PERMISSIONS") {
    await ctx.sql(`UPDATE gmail.sync_cursors SET status = 'needs_reauth' WHERE id = $1`, [sc.id]);
    return;
  }
  if (err.code === "SYNC_CURSOR_ERROR") {
    await ctx.sql(`UPDATE gmail.sync_cursors SET cursor = null, status = 'idle' WHERE id = $1`, [sc.id]);
    return;
  }
  const count = (sc.throttle_count ?? 0) + 1;
  if (count >= MAX_THROTTLE) {
    await ctx.sql(`UPDATE gmail.sync_cursors SET status = 'failed_permanent', throttle_count = $1 WHERE id = $2`, [count, sc.id]);
    return;
  }
  const wait = 60_000 * Math.pow(2, Math.min(count - 1, 5));
  const after = new Date(Math.max(Date.now() + wait, err.retryAfter ?? 0)).toISOString();
  await ctx.sql(
    `UPDATE gmail.sync_cursors SET status = 'failed_temporary', throttle_count = $1, throttle_after = $2 WHERE id = $3`,
    [count, after, sc.id]
  );
}
