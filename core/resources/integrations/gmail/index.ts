/// <reference path="../rootcx-worker.d.ts" />
import { google } from "googleapis";
import {
  type Config, type UserCreds,
  isManaged, oauth2ClientFor, managedAccessToken,
  selfHostedAuthUrl, managedAuthUrl, exchangeCodeForRefreshToken, evictClient,
} from "./lib/oauth";
import { type Result, ok, fail, classifyHttp, withRetry } from "./lib/errors";
import { parseMessage } from "./lib/parse-message";
import { composeMessage, type SendInput } from "./lib/mail-composer";
import { cacheAliases, getCachedAliases } from "./lib/aliases";

const GMAIL_API = "https://www.googleapis.com/gmail/v1/users/me";
const EXCLUDE_QUERY = "-in:spam -in:trash -in:drafts -category:promotions -category:social";
const MAX_THROTTLE = 5;
const FETCH_CONCURRENCY = 15;

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
    __auth_start: authStart,
    __auth_callback: authCallback,
    async __integration(params, caller) {
      const { action, input, config, userCredentials, userId } = params;
      const connected = isManaged(config) ? userCredentials?.managed : userCredentials?.refreshToken;
      if (!connected) return fail({ code: "INSUFFICIENT_PERMISSIONS", message: "user not connected" });
      const handler = actions[action];
      if (!handler) return fail({ code: "MISCONFIGURED", message: `unknown action: ${action}` });
      const token = caller?.authToken ?? "";
      return handler(config, userCredentials, input, userId, token);
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

async function ensureIndexes() {
  if (!db) return;
  await db`CREATE UNIQUE INDEX IF NOT EXISTS idx_gmail_msg_ext ON gmail.messages (external_id)`;
  await db`CREATE UNIQUE INDEX IF NOT EXISTS idx_gmail_msg_hdr ON gmail.messages (header_message_id)`;
  await db`CREATE INDEX IF NOT EXISTS idx_gmail_msg_thread ON gmail.messages (thread_external_id)`;
  await db`CREATE INDEX IF NOT EXISTS idx_gmail_msg_user ON gmail.messages (user_id)`;
  await db`CREATE UNIQUE INDEX IF NOT EXISTS idx_gmail_thread_ext ON gmail.threads (external_id)`;
  await db`CREATE INDEX IF NOT EXISTS idx_gmail_part_msg ON gmail.participants (message_id)`;
  await db`CREATE INDEX IF NOT EXISTS idx_gmail_part_addr ON gmail.participants (lower(address))`;
  await db`CREATE INDEX IF NOT EXISTS idx_gmail_att_msg ON gmail.attachments (message_id)`;
  await db`CREATE UNIQUE INDEX IF NOT EXISTS idx_gmail_cursor_user ON gmail.sync_cursors (user_id)`;
}

async function accessTokenFor(config: Config, creds: UserCreds, userId: string): Promise<string> {
  if (isManaged(config)) return managedAccessToken(config, userId);
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

async function authStart(params: any) {
  const { config, callbackUrl, state, userId } = params;
  return { type: "redirect", url: isManaged(config) ? managedAuthUrl(config, callbackUrl, state, userId) : selfHostedAuthUrl(config, callbackUrl, state) };
}

async function authCallback(params: any) {
  const { config, query } = params;
  if (isManaged(config) || query.code === "MANAGED_OK") return { credentials: { managed: true } };
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
  config: Config, creds: UserCreds, input: SendInput, userId: string, asDraft: boolean, token: string,
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
  if (msgId && token) await persistSingleMessage(token, userId, msgId);

  return asDraft
    ? ok({ draftId: r.data.id, messageId: msgId, threadId: r.data.message?.threadId, headerMessageId: composed.headerMessageId })
    : ok({ messageId: r.data.id, threadId: r.data.threadId, headerMessageId: composed.headerMessageId });
}

const sendEmail = (c: Config, u: UserCreds, i: any, uid: string, t: string) => composeAndSend(c, u, i, uid, false, t);
const createDraft = (c: Config, u: UserCreds, i: any, uid: string, t: string) => composeAndSend(c, u, i, uid, true, t);

async function getAttachment(config: Config, creds: UserCreds, input: any, userId: string, _token: string): Promise<Result<any>> {
  if (!input.messageId || !input.attachmentId) return fail({ code: "MISCONFIGURED", message: "messageId and attachmentId required" });
  return callGmail(config, creds, `/messages/${input.messageId}/attachments/${input.attachmentId}`, undefined, userId);
}

async function syncConnect(config: Config, creds: UserCreds, _input: any, userId: string, token: string): Promise<Result<any>> {
  const aliases = await fetchAndCacheAliases(config, creds, userId);
  if (!aliases.ok) return aliases;
  const { primary: handle, aliases: aliasList } = aliases.data;

  const existing = await db`SELECT id, cron_id FROM gmail.sync_cursors WHERE user_id = ${userId}`;
  let cursorId: string;
  let cronId: string | null = null;
  if (existing.length) {
    cursorId = existing[0].id;
    cronId = existing[0].cron_id;
    await db`UPDATE gmail.sync_cursors SET handle = ${handle}, handle_aliases = ${JSON.stringify(aliasList)}, enabled = true, status = 'idle', throttle_count = 0, throttle_after = null WHERE id = ${cursorId}`;
  } else {
    const ins = await db`INSERT INTO gmail.sync_cursors (user_id, handle, handle_aliases, status, sync_stage, enabled, throttle_count) VALUES (${userId}, ${handle}, ${JSON.stringify(aliasList)}, 'idle', 'list_fetch', true, 0) RETURNING id`;
    cursorId = ins[0].id;
  }

  await syncUserNow(userId, token);
  return ok({ cursor_id: cursorId, handle });
}

async function syncDisconnect(_config: Config, _creds: UserCreds, _input: any, userId: string, _token: string): Promise<Result<any>> {
  await db`UPDATE gmail.sync_cursors SET enabled = false WHERE user_id = ${userId}`;
  return ok({});
}

async function syncNow(_config: Config, _creds: UserCreds, _input: any, userId: string, token: string): Promise<Result<any>> {
  const cursor = await db`SELECT id, status, throttle_after FROM gmail.sync_cursors WHERE user_id = ${userId} AND enabled = true`;
  if (!cursor.length) return fail({ code: "MISCONFIGURED", message: "no active sync" });
  if (cursor[0].status === "syncing") return ok({ triggered: false });
  if (cursor[0].throttle_after && new Date(cursor[0].throttle_after).getTime() > Date.now()) return ok({ triggered: false });
  await syncUserNow(userId, token);
  return ok({ triggered: true });
}

async function syncUserNow(userId: string, token: string) {
  await db`UPDATE gmail.sync_cursors SET status = 'syncing' WHERE user_id = ${userId}`;
  const cursors = await db`SELECT * FROM gmail.sync_cursors WHERE user_id = ${userId} AND enabled = true`;
  if (!cursors.length) return;
  const sc = cursors[0];
  try {
    if (!sc.cursor) await fullSync(token, userId, sc);
    else await incrementalSync(token, userId, sc);
    await db`UPDATE gmail.sync_cursors SET status = 'idle', last_synced_at = NOW(), throttle_count = 0, throttle_after = null WHERE id = ${sc.id}`;
  } catch (e: any) {
    log.error(`sync ${userId}: ${e.message}`);
    await handleSyncError(sc, e);
  }
}

async function runtimeFetch(method: string, path: string, token: string, body?: any, extraHeaders?: Record<string, string>): Promise<any> {
  const res = await fetch(`${ctx.runtimeUrl}${path}`, {
    method,
    headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}`, ...extraHeaders },
    ...(body ? { body: JSON.stringify(body) } : {}),
  });
  if (!res.ok) throw new Error(`${method} ${path} -> ${res.status}`);
  return res.json().catch(() => null);
}

async function getProfile(config: Config, creds: UserCreds, _input: any, userId: string, _token: string): Promise<Result<any>> {
  const r = await callGmail(config, creds, "/profile", undefined, userId);
  if (!r.ok) return r;
  return ok({ emailAddress: r.data.emailAddress, historyId: r.data.historyId });
}

async function listSendAs(config: Config, creds: UserCreds, _input: any, userId: string, _token: string): Promise<Result<any>> {
  const r = await callGmail(config, creds, "/settings/sendAs", undefined, userId);
  if (!r.ok) return r;
  return ok({ sendAs: (r.data.sendAs ?? []).map((e: any) => ({ sendAsEmail: e.sendAsEmail, displayName: e.displayName ?? "", isDefault: !!e.isDefault, isPrimary: !!e.isPrimary })) });
}

async function watchAction(config: Config, creds: UserCreds, input: any, userId: string, _token: string): Promise<Result<any>> {
  const topicName = input?.topicName ?? config.pubsubTopicName;
  if (!topicName) return fail({ code: "MISCONFIGURED", message: "topicName required" });
  const r = await callGmail(config, creds, "/watch", {
    method: "POST", headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ topicName, ...(input?.labelIds ? { labelIds: input.labelIds } : {}) }),
  }, userId);
  if (!r.ok) return r;
  return ok({ historyId: r.data.historyId, expiration: r.data.expiration ? Number(r.data.expiration) : null });
}

async function stopWatch(config: Config, creds: UserCreds, _input: any, userId: string, _token: string): Promise<Result<any>> {
  const r = await callGmail(config, creds, "/stop", { method: "POST", headers: { "Content-Type": "application/json" }, body: "{}" }, userId);
  if (!r.ok) return r;
  return ok({});
}

async function listMessages(config: Config, creds: UserCreds, input: any, userId: string, _token: string): Promise<Result<any>> {
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

async function getEmail(config: Config, creds: UserCreds, input: any, userId: string, _token: string): Promise<Result<any>> {
  if (!input?.messageId) return fail({ code: "MISCONFIGURED", message: "messageId required" });
  const r = await callGmail(config, creds, `/messages/${input.messageId}?format=full`, undefined, userId);
  if (!r.ok) return r;
  return ok(parseMessage(r.data));
}

async function historyList(config: Config, creds: UserCreds, input: any, userId: string, _token: string): Promise<Result<any>> {
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

const actions: Record<string, (c: Config, u: UserCreds, i: any, uid: string, token: string) => Promise<Result<any>>> = {
  send_email: sendEmail, create_draft: createDraft, get_attachment: getAttachment,
  sync_connect: syncConnect, sync_disconnect: syncDisconnect, sync_now: syncNow,
  get_profile: getProfile, list_send_as: listSendAs, watch: watchAction, stop_watch: stopWatch,
  list_messages: listMessages, get_email: getEmail, history_list: historyList,
};

async function handleJob(payload: any, caller: any) {
  if (payload?.type === "sync_all") return syncAllConnectedUsers(caller, "sync_now");
  if (payload?.type === "sync") {
    const token: string = caller?.authToken;
    if (!token) return { error: "no auth token in job caller" };
    await syncUserNow(payload.user_id, token);
    return { ok: true };
  }
  return { skipped: true };
}

async function selfAction(token: string, action: string, input: any): Promise<any> {
  const res = await fetch(`${ctx.runtimeUrl}/api/v1/integrations/gmail/actions/${action}`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}` },
    body: JSON.stringify(input),
  });
  if (!res.ok) {
    const text = await res.text().catch(() => "");
    throw new Error(`gmail/${action} -> ${res.status}: ${text.slice(0, 200)}`);
  }
  const result = await res.json();
  if (result?.ok === false && result?.error) {
    const e: any = new Error(result.error.message ?? action);
    e.code = result.error.code;
    e.retryAfter = result.error.retryAfter;
    throw e;
  }
  return result?.ok === true && "data" in result ? result.data : result;
}

async function fullSync(token: string, userId: string, sc: any) {
  const profile = await selfAction(token, "get_profile", {});
  const snapshotCursor = profile.historyId;

  let pageToken: string | undefined;
  do {
    const r = await selfAction(token, "list_messages", { query: EXCLUDE_QUERY, maxResults: 500, pageToken });
    const ids: string[] = (r.messages ?? []).map((m: any) => m.id);
    await importBatch(token, userId, ids);
    pageToken = r.nextPageToken ?? undefined;
  } while (pageToken);

  await db`UPDATE gmail.sync_cursors SET cursor = ${snapshotCursor}, sync_stage = 'list_fetch' WHERE id = ${sc.id}`;
}

async function incrementalSync(token: string, userId: string, sc: any) {
  let pageToken: string | undefined;
  let latestHistoryId: string | null = null;
  do {
    let r: any;
    try {
      r = await selfAction(token, "history_list", { startHistoryId: sc.cursor, maxResults: 500, pageToken });
    } catch (e: any) {
      if (e.code === "SYNC_CURSOR_ERROR") {
        await db`UPDATE gmail.sync_cursors SET cursor = null WHERE id = ${sc.id}`;
        return fullSync(token, userId, { ...sc, cursor: null });
      }
      throw e;
    }
    const added: string[] = r.messagesAdded ?? [];
    const deleted: string[] = r.messagesDeleted ?? [];
    if (deleted.length) await db`DELETE FROM gmail.messages WHERE external_id = ANY(${deleted}) AND user_id = ${userId}`;
    if (added.length) await importBatch(token, userId, added);
    latestHistoryId = r.historyId ?? latestHistoryId;
    pageToken = r.nextPageToken ?? undefined;
  } while (pageToken);

  if (latestHistoryId) await db`UPDATE gmail.sync_cursors SET cursor = ${latestHistoryId} WHERE id = ${sc.id}`;
}

async function importBatch(token: string, userId: string, externalIds: string[]) {
  if (!externalIds.length) return;
  const existing = await db`SELECT external_id FROM gmail.messages WHERE external_id = ANY(${externalIds})`;
  const existingSet = new Set(existing.map((r: any) => r.external_id));
  const missing = externalIds.filter(id => !existingSet.has(id));
  if (!missing.length) return;

  for (let i = 0; i < missing.length; i += FETCH_CONCURRENCY) {
    const chunk = missing.slice(i, i + FETCH_CONCURRENCY);
    await Promise.allSettled(chunk.map(id => persistSingleMessage(token, userId, id)));
  }
}

async function persistSingleMessage(token: string, userId: string, externalId: string) {
  let msg: any;
  try { msg = await selfAction(token, "get_email", { messageId: externalId }); }
  catch { return; }

  const headerMsgId = msg.headerMessageId || `fallback-${userId}-${externalId}`;

  const inserted = await db`
    INSERT INTO gmail.messages (external_id, thread_external_id, header_message_id, user_id, subject, body_text, body_html, snippet, internal_date, label_ids, in_reply_to, "references")
    VALUES (${externalId}, ${msg.threadId}, ${headerMsgId}, ${userId}, ${msg.subject}, ${msg.bodyText}, ${msg.bodyHtml}, ${msg.snippet}, ${msg.internalDate ? new Date(msg.internalDate).toISOString() : null}, ${JSON.stringify(msg.labelIds)}, ${msg.inReplyTo}, ${JSON.stringify(msg.references)})
    ON CONFLICT (external_id) DO NOTHING
    RETURNING id
  `;
  if (!inserted.length) return;
  const msgDbId = inserted[0].id;

  if (msg.threadId) {
    await db`
      INSERT INTO gmail.threads (external_id, user_id, subject, last_message_at, message_count)
      VALUES (${msg.threadId}, ${userId}, ${msg.subject}, ${msg.internalDate ? new Date(msg.internalDate).toISOString() : null}, 1)
      ON CONFLICT (external_id) DO UPDATE SET
        last_message_at = GREATEST(gmail.threads.last_message_at, EXCLUDED.last_message_at),
        message_count = gmail.threads.message_count + 1
    `;
  }

  const parts: Array<{ message_id: string; address: string; name: string; role: string }> = [];
  if (msg.from) parts.push({ message_id: msgDbId, role: "from", address: msg.from.address, name: msg.from.name });
  for (const p of msg.to) parts.push({ message_id: msgDbId, role: "to", address: p.address, name: p.name });
  for (const p of msg.cc) parts.push({ message_id: msgDbId, role: "cc", address: p.address, name: p.name });
  for (const p of msg.bcc) parts.push({ message_id: msgDbId, role: "bcc", address: p.address, name: p.name });
  if (parts.length) {
    await db`INSERT INTO gmail.participants ${db(parts)} ON CONFLICT DO NOTHING`;
  }

  if (msg.attachments.length) {
    const attRows = msg.attachments.map(a => ({
      message_id: msgDbId, external_id: a.id, filename: a.filename,
      mime_type: a.mimeType, size: a.size, content_id: a.contentId, is_inline: a.isInline,
    }));
    await db`INSERT INTO gmail.attachments ${db(attRows)} ON CONFLICT DO NOTHING`;
  }
}

async function handleSyncError(sc: any, err: any) {
  if (err.code === "INSUFFICIENT_PERMISSIONS") {
    await db`UPDATE gmail.sync_cursors SET status = 'needs_reauth' WHERE id = ${sc.id}`;
    return;
  }
  if (err.code === "SYNC_CURSOR_ERROR") {
    await db`UPDATE gmail.sync_cursors SET cursor = null, status = 'idle' WHERE id = ${sc.id}`;
    return;
  }
  const count = (sc.throttle_count ?? 0) + 1;
  if (count >= MAX_THROTTLE) {
    await db`UPDATE gmail.sync_cursors SET status = 'failed_permanent', throttle_count = ${count} WHERE id = ${sc.id}`;
    return;
  }
  const wait = 60_000 * Math.pow(2, Math.min(count - 1, 5));
  const after = new Date(Math.max(Date.now() + wait, err.retryAfter ?? 0)).toISOString();
  await db`UPDATE gmail.sync_cursors SET status = 'failed_temporary', throttle_count = ${count}, throttle_after = ${after} WHERE id = ${sc.id}`;
}
