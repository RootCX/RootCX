/// <reference path="../rootcx-worker.d.ts" />
import {
  type Config, type UserCreds,
  oauth2ClientFor, authUrl, exchangeCodeForRefreshToken, evictClient,
} from "./lib/oauth";
import { type Result, ok, fail, classifyHttp, withRetry } from "./lib/errors";
import { parseEvent, type ParsedEvent } from "./lib/parse-event";
import { composeEvent, type ComposeInput } from "./lib/compose-event";
import { cryptoRandomId, jsonReq, calId, eventsPath, eventPath } from "./lib/util";

const CALENDAR_API = "https://www.googleapis.com/calendar/v3";
const MAX_THROTTLE = 5;
const PAGE_SIZE = 250;

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

async function accessTokenFor(config: Config, creds: UserCreds, userId: string): Promise<string> {
  const access = await oauth2ClientFor(config, creds, userId).getAccessToken();
  if (!access.token) throw new Error("no access token from refresh");
  return access.token;
}

async function calApi(
  config: Config, creds: UserCreds, path: string, init: RequestInit | undefined, userId: string,
): Promise<Result<any>> {
  let token: string;
  try { token = await accessTokenFor(config, creds, userId); }
  catch (e: any) { evictClient(userId); return fail({ code: "INSUFFICIENT_PERMISSIONS", message: e?.message ?? "auth failed" }); }
  let res: Response;
  try { res = await fetch(`${CALENDAR_API}${path}`, { ...init, headers: { Authorization: `Bearer ${token}`, ...init?.headers } }); }
  catch (e: any) { return fail({ code: "TEMPORARY_ERROR", message: e?.message ?? "network error" }); }
  if (!res.ok) {
    const body = await res.text().catch(() => "");
    const err = classifyHttp(res.status, body);
    if (err.code === "INSUFFICIENT_PERMISSIONS") evictClient(userId);
    return fail(err);
  }
  if (res.status === 204) return ok({});
  return ok(await res.json());
}

const callCal = (config: Config, creds: UserCreds, path: string, init: RequestInit | undefined, userId: string) =>
  withRetry(() => calApi(config, creds, path, init, userId));

async function authStart(params: any) {
  const { config, callbackUrl, state } = params;
  return { type: "redirect", url: authUrl(config, callbackUrl, state) };
}

async function authCallback(params: any) {
  const { config, query } = params;
  const refreshToken = await exchangeCodeForRefreshToken(config, query.code, query.redirect_uri ?? params.callbackUrl ?? "");
  return { credentials: { refreshToken } };
}

async function getProfile(config: Config, creds: UserCreds, _input: any, userId: string, _ctx: RootCxCtx): Promise<Result<any>> {
  const cals = await callCal(config, creds, "/users/me/calendarList?maxResults=250", undefined, userId);
  if (!cals.ok) return cals;
  const primary = (cals.data.items ?? []).find((c: any) => c.primary);
  if (!primary) return fail({ code: "NOT_FOUND", message: "primary calendar not found" });
  return ok({ emailAddress: (primary.id ?? "").toLowerCase(), timeZone: primary.timeZone ?? "" });
}

async function listCalendars(config: Config, creds: UserCreds, input: any, userId: string, _ctx: RootCxCtx): Promise<Result<any>> {
  const params = new URLSearchParams({ maxResults: "250" });
  if (input?.showHidden) params.set("showHidden", "true");
  if (input?.minAccessRole) params.set("minAccessRole", String(input.minAccessRole));
  const all: any[] = [];
  let pageToken: string | undefined;
  do {
    if (pageToken) params.set("pageToken", pageToken); else params.delete("pageToken");
    const r = await callCal(config, creds, `/users/me/calendarList?${params}`, undefined, userId);
    if (!r.ok) return r;
    for (const it of r.data.items ?? []) all.push(it);
    pageToken = r.data.nextPageToken;
  } while (pageToken);
  return ok({
    calendars: all.map(it => ({
      id: it.id,
      summary: it.summaryOverride ?? it.summary ?? "",
      description: it.description ?? "",
      primary: !!it.primary,
      timeZone: it.timeZone ?? "",
      accessRole: it.accessRole ?? "",
      backgroundColor: it.backgroundColor ?? "",
      foregroundColor: it.foregroundColor ?? "",
      selected: it.selected !== false,
      hidden: !!it.hidden,
      deleted: !!it.deleted,
    })),
  });
}

async function listEvents(config: Config, creds: UserCreds, input: any, userId: string, _ctx: RootCxCtx): Promise<Result<any>> {
  const p = new URLSearchParams();
  if (input?.syncToken) {
    p.set("syncToken", input.syncToken);
    p.set("showDeleted", "true");
  } else {
    if (input?.timeMin) p.set("timeMin", input.timeMin);
    if (input?.timeMax) p.set("timeMax", input.timeMax);
    if (input?.q) p.set("q", String(input.q).slice(0, 1024));
    if (input?.orderBy) p.set("orderBy", input.orderBy);
    if (input?.showDeleted) p.set("showDeleted", "true");
  }
  if (input?.pageToken) p.set("pageToken", input.pageToken);
  if (input?.singleEvents !== false) p.set("singleEvents", "true");
  p.set("maxResults", String(Math.min(Math.max(1, input?.maxResults ?? PAGE_SIZE), 2500)));
  const r = await callCal(config, creds, `${eventsPath(input)}?${p}`, undefined, userId);
  if (!r.ok) return r;
  return ok({
    events: (r.data.items ?? []).map(parseEvent),
    nextPageToken: r.data.nextPageToken ?? null,
    nextSyncToken: r.data.nextSyncToken ?? null,
  });
}

async function getEvent(config: Config, creds: UserCreds, input: any, userId: string, _ctx: RootCxCtx): Promise<Result<any>> {
  if (!input?.eventId) return fail({ code: "MISCONFIGURED", message: "eventId required" });
  const r = await callCal(config, creds, eventPath(input), undefined, userId);
  if (!r.ok) return r;
  return ok(parseEvent(r.data));
}

async function createEvent(config: Config, creds: UserCreds, input: any, userId: string, _ctx: RootCxCtx): Promise<Result<any>> {
  let composed;
  try { composed = composeEvent(input as ComposeInput); }
  catch (e: any) { return fail({ code: "MISCONFIGURED", message: e?.message ?? "invalid input" }); }
  const p = new URLSearchParams({ sendUpdates: input.sendUpdates ?? "all" });
  if (composed.conferenceDataVersion === 1) p.set("conferenceDataVersion", "1");
  const r = await callCal(config, creds, `${eventsPath(input)}?${p}`, jsonReq("POST", composed.body), userId);
  if (!r.ok) return r;
  return ok({ eventId: r.data.id, htmlLink: r.data.htmlLink, hangoutLink: r.data.hangoutLink ?? "" });
}

async function updateEvent(config: Config, creds: UserCreds, input: any, userId: string, _ctx: RootCxCtx): Promise<Result<any>> {
  if (!input?.eventId || !input?.patch) return fail({ code: "MISCONFIGURED", message: "eventId and patch required" });
  const p = new URLSearchParams({ sendUpdates: input.sendUpdates ?? "all" });
  const r = await callCal(config, creds, `${eventPath(input)}?${p}`, jsonReq("PATCH", input.patch), userId);
  if (!r.ok) return r;
  return ok(parseEvent(r.data));
}

async function deleteEvent(config: Config, creds: UserCreds, input: any, userId: string, _ctx: RootCxCtx): Promise<Result<any>> {
  if (!input?.eventId) return fail({ code: "MISCONFIGURED", message: "eventId required" });
  const p = new URLSearchParams({ sendUpdates: input.sendUpdates ?? "all" });
  const r = await callCal(config, creds, `${eventPath(input)}?${p}`, { method: "DELETE" }, userId);
  if (!r.ok) return r;
  return ok({ ok: true });
}

async function respondEvent(config: Config, creds: UserCreds, input: any, userId: string, _ctx: RootCxCtx): Promise<Result<any>> {
  if (!input?.eventId || !input?.responseStatus) return fail({ code: "MISCONFIGURED", message: "eventId and responseStatus required" });
  const getR = await callCal(config, creds, eventPath(input), undefined, userId);
  if (!getR.ok) return getR;
  const attendees = (getR.data.attendees ?? []).map((a: any) =>
    a.self ? { ...a, responseStatus: input.responseStatus } : a,
  );
  if (!attendees.some((a: any) => a.self)) {
    return fail({ code: "NOT_FOUND", message: "current user is not an attendee" });
  }
  const r = await callCal(config, creds, `${eventPath(input)}?sendUpdates=none`, jsonReq("PATCH", { attendees }), userId);
  if (!r.ok) return r;
  return ok(parseEvent(r.data));
}

async function quickAdd(config: Config, creds: UserCreds, input: any, userId: string, _ctx: RootCxCtx): Promise<Result<any>> {
  if (!input?.text) return fail({ code: "MISCONFIGURED", message: "text required" });
  const p = new URLSearchParams({ text: input.text, sendUpdates: input.sendUpdates ?? "all" });
  const r = await callCal(config, creds, `${eventsPath(input)}/quickAdd?${p}`, { method: "POST" }, userId);
  if (!r.ok) return r;
  return ok({ eventId: r.data.id, htmlLink: r.data.htmlLink });
}

async function freebusy(config: Config, creds: UserCreds, input: any, userId: string, _ctx: RootCxCtx): Promise<Result<any>> {
  if (!input?.timeMin || !input?.timeMax || !Array.isArray(input?.items)) {
    return fail({ code: "MISCONFIGURED", message: "timeMin, timeMax and items required" });
  }
  if (input.items.length > 50) return fail({ code: "MISCONFIGURED", message: "max 50 calendars per freebusy query" });
  const body: any = { timeMin: input.timeMin, timeMax: input.timeMax, items: input.items };
  if (input.timeZone) body.timeZone = input.timeZone;
  const r = await callCal(config, creds, "/freeBusy", jsonReq("POST", body), userId);
  if (!r.ok) return r;
  return ok({ calendars: r.data.calendars ?? {} });
}

async function watchAction(config: Config, creds: UserCreds, input: any, userId: string, _ctx: RootCxCtx): Promise<Result<any>> {
  if (!input?.address) return fail({ code: "MISCONFIGURED", message: "address required" });
  const body: any = { id: cryptoRandomId(), type: "web_hook", address: input.address };
  if (input.token) body.token = input.token;
  if (input.ttlSeconds) body.params = { ttl: String(input.ttlSeconds) };
  const r = await callCal(config, creds, `/calendars/${calId(input)}/events/watch`, jsonReq("POST", body), userId);
  if (!r.ok) return r;
  return ok({ channelId: r.data.id, resourceId: r.data.resourceId, expiration: r.data.expiration ? Number(r.data.expiration) : null });
}

async function updateCalendarVisibility(_config: Config, _creds: UserCreds, input: any, userId: string, ctx: RootCxCtx): Promise<Result<any>> {
  if (!input?.calendarExternalId || !input?.visibility) {
    return fail({ code: "MISCONFIGURED", message: "calendarExternalId and visibility required" });
  }
  if (input.visibility !== "share_everything" && input.visibility !== "metadata") {
    return fail({ code: "MISCONFIGURED", message: "visibility must be share_everything or metadata" });
  }
  const updated = await ctx.sql(
    `UPDATE google_calendar.calendars SET visibility = $1 WHERE user_id = $2 AND external_id = $3 RETURNING id`,
    [input.visibility, userId, input.calendarExternalId]
  );
  if (!updated.rows.length) return fail({ code: "NOT_FOUND", message: "calendar not found for this user" });
  return ok({ ok: true });
}

async function stopWatch(config: Config, creds: UserCreds, input: any, userId: string, _ctx: RootCxCtx): Promise<Result<any>> {
  if (!input?.channelId || !input?.resourceId) return fail({ code: "MISCONFIGURED", message: "channelId and resourceId required" });
  const r = await callCal(config, creds, "/channels/stop", jsonReq("POST", { id: input.channelId, resourceId: input.resourceId }), userId);
  if (!r.ok) return r;
  return ok({});
}

async function syncConnect(config: Config, creds: UserCreds, _input: any, userId: string, ctx: RootCxCtx): Promise<Result<any>> {
  const cals = await listCalendars(config, creds, {}, userId);
  if (!cals.ok) return cals;

  const selected = cals.data.calendars.filter((c: any) => c.selected && !c.deleted);
  if (!selected.length) return fail({ code: "MISCONFIGURED", message: "no selected calendars" });
  const selectedIds = selected.map((c: any) => c.id);

  if (selected.length) {
    const cols = 10;
    const values = selected.map((_: any, i: number) => `($${i*cols+1}, $${i*cols+2}, $${i*cols+3}, $${i*cols+4}, $${i*cols+5}, $${i*cols+6}, $${i*cols+7}, $${i*cols+8}, $${i*cols+9}, $${i*cols+10}, 'share_everything')`).join(", ");
    const params = selected.flatMap((c: any) => [c.id, userId, c.summary, c.description, c.primary, c.timeZone, c.accessRole, c.backgroundColor, c.foregroundColor, c.selected]);
    await ctx.sql(
      `INSERT INTO google_calendar.calendars (external_id, user_id, summary, description, "primary", time_zone, access_role, background_color, foreground_color, selected, visibility)
       VALUES ${values}
       ON CONFLICT (user_id, external_id) DO UPDATE SET
         summary = EXCLUDED.summary, description = EXCLUDED.description, "primary" = EXCLUDED."primary",
         time_zone = EXCLUDED.time_zone, access_role = EXCLUDED.access_role,
         background_color = EXCLUDED.background_color, foreground_color = EXCLUDED.foreground_color,
         selected = EXCLUDED.selected`,
      params
    );
  }

  await ctx.sql(
    `UPDATE google_calendar.sync_cursors SET enabled = false WHERE user_id = $1 AND calendar_external_id != ALL($2)`,
    [userId, selectedIds]
  );

  const cols = 2;
  const cursorValues = selected.map((_: any, i: number) => `($${i*cols+1}, $${i*cols+2}, 'idle', true, 0)`).join(", ");
  const cursorParams = selected.flatMap((c: any) => [userId, c.id]);
  const upserted = await ctx.sql(
    `INSERT INTO google_calendar.sync_cursors (user_id, calendar_external_id, status, enabled, throttle_count)
     VALUES ${cursorValues}
     ON CONFLICT (user_id, calendar_external_id) DO UPDATE SET
       enabled = true, status = 'idle', throttle_count = 0, throttle_after = null
     RETURNING id`,
    cursorParams
  );
  const cursorIds = upserted.rows.map((r: any) => r[0]);

  await syncUserNow(userId, ctx);
  return ok({ cursor_ids: cursorIds, calendars: selected.map((c: any) => ({ id: c.id, summary: c.summary })) });
}

async function syncDisconnect(_config: Config, _creds: UserCreds, _input: any, userId: string, ctx: RootCxCtx): Promise<Result<any>> {
  await ctx.sql(`UPDATE google_calendar.sync_cursors SET enabled = false WHERE user_id = $1`, [userId]);
  return ok({ ok: true });
}

async function syncNow(_config: Config, _creds: UserCreds, _input: any, userId: string, ctx: RootCxCtx): Promise<Result<any>> {
  const cursors = await ctx.sql(
    `SELECT id, status, throttle_after FROM google_calendar.sync_cursors WHERE user_id = $1 AND enabled = true`, [userId]
  );
  if (!cursors.rows.length) return fail({ code: "MISCONFIGURED", message: "no active sync" });
  const blocked = cursors.rows.every((r: any) =>
    r[1] === "syncing" || (r[2] && new Date(r[2]).getTime() > Date.now()),
  );
  if (blocked) return ok({ triggered: false });
  await syncUserNow(userId, ctx);
  return ok({ triggered: true });
}

async function syncUserNow(userId: string, ctx: RootCxCtx) {
  await ctx.sql(
    `UPDATE google_calendar.sync_cursors SET status = 'syncing' WHERE user_id = $1 AND enabled = true AND status != 'needs_reauth'`, [userId]
  );
  const cursors = await ctx.sql(
    `SELECT id, calendar_external_id, sync_token, throttle_after, throttle_count FROM google_calendar.sync_cursors WHERE user_id = $1 AND enabled = true`, [userId]
  );
  for (const row of cursors.rows) {
    const [id, calendar_external_id, sync_token, throttle_after, throttle_count] = row as any[];
    if (throttle_after && new Date(throttle_after).getTime() > Date.now()) continue;
    const sc = { id, calendar_external_id, sync_token, throttle_count };
    try {
      await syncCursor(ctx, userId, sc);
      await ctx.sql(
        `UPDATE google_calendar.sync_cursors SET status = 'idle', last_synced_at = NOW(), throttle_count = 0, throttle_after = null WHERE id = $1`, [sc.id]
      );
    } catch (e: any) {
      log.error(`sync ${userId}/${sc.calendar_external_id}: ${e.message}`);
      await handleSyncError(ctx, sc, e);
    }
  }
}

const actions: Record<string, (c: Config, u: UserCreds, i: any, uid: string, ctx: RootCxCtx) => Promise<Result<any>>> = {
  sync_connect: syncConnect, sync_disconnect: syncDisconnect, sync_now: syncNow,
  get_profile: getProfile, list_calendars: listCalendars,
  update_calendar_visibility: updateCalendarVisibility,
  list_events: listEvents, get_event: getEvent,
  create_event: createEvent, update_event: updateEvent, delete_event: deleteEvent,
  respond_event: respondEvent, quick_add: quickAdd, freebusy: freebusy,
  watch: watchAction, stop_watch: stopWatch,
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

async function syncCursor(ctx: RootCxCtx, userId: string, sc: any) {
  let pageToken: string | undefined;
  let nextSyncToken: string | undefined;
  const calendarId = sc.calendar_external_id;

  while (true) {
    const args: any = {
      calendarId,
      maxResults: 250,
      singleEvents: true,
      pageToken,
    };
    if (sc.sync_token) args.syncToken = sc.sync_token;
    else args.showDeleted = true;

    let data: any;
    try { data = await runAction(ctx, "list_events", args); }
    catch (e: any) {
      if (e.code === "SYNC_CURSOR_ERROR") {
        await ctx.sql(`UPDATE google_calendar.sync_cursors SET sync_token = null WHERE id = $1`, [sc.id]);
        return syncCursor(ctx, userId, { ...sc, sync_token: null });
      }
      throw e;
    }

    const events: ParsedEvent[] = data.events ?? [];
    const cancelled: string[] = [];
    const upserts: ParsedEvent[] = [];
    for (const ev of events) {
      if (ev.isCanceled) { if (ev.externalId) cancelled.push(ev.externalId); }
      else upserts.push(ev);
    }

    if (cancelled.length) {
      const orphan = await ctx.sql(
        `SELECT event_id FROM google_calendar.channel_event_associations
         WHERE user_id = $1 AND calendar_external_id = $2 AND event_external_id = ANY($3)`,
        [userId, calendarId, cancelled]
      );
      await ctx.sql(
        `DELETE FROM google_calendar.channel_event_associations
         WHERE user_id = $1 AND calendar_external_id = $2 AND event_external_id = ANY($3)`,
        [userId, calendarId, cancelled]
      );
      const orphanIds = orphan.rows.map((r: any) => r[0]);
      if (orphanIds.length) {
        await ctx.sql(
          `DELETE FROM google_calendar.events
           WHERE id = ANY($1)
             AND NOT EXISTS (
               SELECT 1 FROM google_calendar.channel_event_associations a WHERE a.event_id = google_calendar.events.id
             )`,
          [orphanIds]
        );
      }
    }

    for (const ev of upserts) await upsertEvent(ctx, userId, calendarId, ev);

    pageToken = data.nextPageToken ?? undefined;
    nextSyncToken = data.nextSyncToken ?? nextSyncToken;
    if (!pageToken) break;
  }

  if (nextSyncToken) {
    await ctx.sql(`UPDATE google_calendar.sync_cursors SET sync_token = $1 WHERE id = $2`, [nextSyncToken, sc.id]);
  }
}

async function upsertEvent(ctx: RootCxCtx, userId: string, calId: string, e: ParsedEvent) {
  if (!e.externalId) return;
  const toIso = (ms: number | null) => ms ? new Date(ms).toISOString() : null;

  const upserted = await ctx.sql(
    `INSERT INTO google_calendar.events (
       external_id, ical_uid, title, description, location,
       starts_at, ends_at, is_full_day, time_zone,
       external_created_at, external_updated_at,
       recurring_event_external_id, recurrence,
       conference_solution, conference_link, html_link, organizer_address,
       transparency, visibility
     ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19)
     ON CONFLICT (external_id) DO UPDATE SET
       ical_uid = EXCLUDED.ical_uid, title = EXCLUDED.title,
       description = EXCLUDED.description, location = EXCLUDED.location,
       starts_at = EXCLUDED.starts_at, ends_at = EXCLUDED.ends_at,
       is_full_day = EXCLUDED.is_full_day, time_zone = EXCLUDED.time_zone,
       external_created_at = EXCLUDED.external_created_at, external_updated_at = EXCLUDED.external_updated_at,
       recurring_event_external_id = EXCLUDED.recurring_event_external_id, recurrence = EXCLUDED.recurrence,
       conference_solution = EXCLUDED.conference_solution, conference_link = EXCLUDED.conference_link,
       html_link = EXCLUDED.html_link, organizer_address = EXCLUDED.organizer_address,
       transparency = EXCLUDED.transparency, visibility = EXCLUDED.visibility
     WHERE google_calendar.events.external_updated_at IS NULL
        OR EXCLUDED.external_updated_at IS NULL
        OR EXCLUDED.external_updated_at >= google_calendar.events.external_updated_at
     RETURNING id`,
    [e.externalId, e.iCalUid || null, e.title, e.description, e.location,
     toIso(e.startsAt), toIso(e.endsAt), e.isFullDay, e.timeZone,
     toIso(e.externalCreatedAt), toIso(e.externalUpdatedAt),
     e.recurringEventExternalId, JSON.stringify(e.recurrence),
     e.conferenceSolution, e.conferenceLink, e.htmlLink, e.organizerAddress,
     e.transparency, e.visibility]
  );
  if (!upserted.rows.length) return;
  const eventId = upserted.rows[0][0];

  await ctx.sql(
    `INSERT INTO google_calendar.channel_event_associations
       (event_id, user_id, calendar_external_id, event_external_id, recurring_event_external_id)
     VALUES ($1, $2, $3, $4, $5)
     ON CONFLICT (user_id, calendar_external_id, event_external_id) DO NOTHING`,
    [eventId, userId, calId, e.externalId, e.recurringEventExternalId]
  );

  if (e.attendees.length) {
    const aCols = 6;
    const aValues = e.attendees.map((_: any, i: number) => `($${i*aCols+1}, $${i*aCols+2}, $${i*aCols+3}, $${i*aCols+4}, $${i*aCols+5}, $${i*aCols+6})`).join(", ");
    const aParams = e.attendees.flatMap((a: any) => [eventId, a.address, a.displayName, a.responseStatus, a.isOrganizer, a.optional]);
    await ctx.sql(
      `INSERT INTO google_calendar.attendees (event_id, address, display_name, response_status, is_organizer, optional)
       VALUES ${aValues}
       ON CONFLICT (event_id, lower(address)) DO UPDATE SET
         display_name = EXCLUDED.display_name, response_status = EXCLUDED.response_status,
         is_organizer = EXCLUDED.is_organizer, optional = EXCLUDED.optional`,
      aParams
    );
    const keepAddrs = e.attendees.map((a: any) => a.address.toLowerCase());
    await ctx.sql(
      `DELETE FROM google_calendar.attendees WHERE event_id = $1 AND lower(address) != ALL($2)`,
      [eventId, keepAddrs]
    );
  } else {
    await ctx.sql(`DELETE FROM google_calendar.attendees WHERE event_id = $1`, [eventId]);
  }

  const attachmentsWithFileId = e.attachments.filter((a: any) => a.fileId);
  if (attachmentsWithFileId.length) {
    const atCols = 6;
    const atValues = attachmentsWithFileId.map((_: any, i: number) => `($${i*atCols+1}, $${i*atCols+2}, $${i*atCols+3}, $${i*atCols+4}, $${i*atCols+5}, $${i*atCols+6})`).join(", ");
    const atParams = attachmentsWithFileId.flatMap((a: any) => [eventId, a.fileId, a.fileUrl, a.title, a.mimeType, a.iconLink]);
    await ctx.sql(
      `INSERT INTO google_calendar.event_attachments (event_id, file_id, file_url, title, mime_type, icon_link)
       VALUES ${atValues}
       ON CONFLICT (event_id, file_id) WHERE file_id IS NOT NULL AND file_id <> '' DO UPDATE SET
         file_url = EXCLUDED.file_url, title = EXCLUDED.title,
         mime_type = EXCLUDED.mime_type, icon_link = EXCLUDED.icon_link`,
      atParams
    );
    const keepFileIds = attachmentsWithFileId.map((a: any) => a.fileId);
    await ctx.sql(
      `DELETE FROM google_calendar.event_attachments WHERE event_id = $1 AND file_id IS NOT NULL AND file_id <> '' AND file_id != ALL($2)`,
      [eventId, keepFileIds]
    );
  } else {
    await ctx.sql(
      `DELETE FROM google_calendar.event_attachments WHERE event_id = $1 AND file_id IS NOT NULL AND file_id <> ''`,
      [eventId]
    );
  }
}

async function handleSyncError(ctx: RootCxCtx, sc: any, err: any) {
  if (err.code === "INSUFFICIENT_PERMISSIONS") {
    await ctx.sql(`UPDATE google_calendar.sync_cursors SET status = 'needs_reauth' WHERE id = $1`, [sc.id]);
    return;
  }
  if (err.code === "SYNC_CURSOR_ERROR") {
    await ctx.sql(`UPDATE google_calendar.sync_cursors SET sync_token = null, status = 'idle' WHERE id = $1`, [sc.id]);
    return;
  }
  const count = (sc.throttle_count ?? 0) + 1;
  if (count >= MAX_THROTTLE) {
    await ctx.sql(`UPDATE google_calendar.sync_cursors SET status = 'failed_permanent', throttle_count = $1 WHERE id = $2`, [count, sc.id]);
    return;
  }
  const wait = 60_000 * Math.pow(2, Math.min(count - 1, 5));
  const after = new Date(Math.max(Date.now() + wait, err.retryAfter ?? 0)).toISOString();
  await ctx.sql(
    `UPDATE google_calendar.sync_cursors SET status = 'failed_temporary', throttle_count = $1, throttle_after = $2 WHERE id = $3`,
    [count, after, sc.id]
  );
}

