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

let ctx: RootCxCtx;
let db: any = null;

serve({
  async onStart(c) {
    ctx = c;
    const postgres = (await import("postgres")).default;
    db = postgres(c.databaseUrl, { max: 10, idle_timeout: 30 });
    await ensureIndexes();
  },
  rpc: {
    __auth_start: authStart,
    __auth_callback: authCallback,
    async __integration(params, caller) {
      const { action, input, config, userCredentials, userId } = params;
      if (!userCredentials?.refreshToken) return fail({ code: "INSUFFICIENT_PERMISSIONS", message: "user not connected" });
      const handler = actions[action];
      if (!handler) return fail({ code: "MISCONFIGURED", message: `unknown action: ${action}` });
      const token = caller?.authToken ?? "";
      return handler(config, userCredentials, input, userId, token);
    },
  },
  onJob: handleJob,
});

async function ensureIndexes() {
  if (!db) return;
  await db`CREATE UNIQUE INDEX IF NOT EXISTS idx_gcal_cal ON google_calendar.calendars (user_id, external_id)`;
  await db`DROP INDEX IF EXISTS google_calendar.idx_gcal_event_ical`;
  await db`CREATE UNIQUE INDEX IF NOT EXISTS idx_gcal_event_external ON google_calendar.events (external_id)`;
  await db`CREATE INDEX IF NOT EXISTS idx_gcal_event_ical_lookup ON google_calendar.events (ical_uid)`;
  await db`CREATE INDEX IF NOT EXISTS idx_gcal_event_starts ON google_calendar.events (starts_at)`;
  await db`CREATE UNIQUE INDEX IF NOT EXISTS idx_gcal_assoc_unique ON google_calendar.channel_event_associations (user_id, calendar_external_id, event_external_id)`;
  await db`CREATE INDEX IF NOT EXISTS idx_gcal_assoc_event ON google_calendar.channel_event_associations (event_id)`;
  await db`CREATE UNIQUE INDEX IF NOT EXISTS idx_gcal_attendee_unique ON google_calendar.attendees (event_id, lower(address))`;
  await db`CREATE INDEX IF NOT EXISTS idx_gcal_attendee_addr ON google_calendar.attendees (lower(address))`;
  await db`CREATE UNIQUE INDEX IF NOT EXISTS idx_gcal_attach_unique ON google_calendar.event_attachments (event_id, file_id) WHERE file_id IS NOT NULL AND file_id <> ''`;
  await db`CREATE INDEX IF NOT EXISTS idx_gcal_attach_event ON google_calendar.event_attachments (event_id)`;
  await db`CREATE UNIQUE INDEX IF NOT EXISTS idx_gcal_cursor_unique ON google_calendar.sync_cursors (user_id, calendar_external_id)`;
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

async function getProfile(config: Config, creds: UserCreds, _input: any, userId: string): Promise<Result<any>> {
  const cals = await callCal(config, creds, "/users/me/calendarList?maxResults=250", undefined, userId);
  if (!cals.ok) return cals;
  const primary = (cals.data.items ?? []).find((c: any) => c.primary);
  if (!primary) return fail({ code: "NOT_FOUND", message: "primary calendar not found" });
  return ok({ emailAddress: (primary.id ?? "").toLowerCase(), timeZone: primary.timeZone ?? "" });
}

async function listCalendars(config: Config, creds: UserCreds, input: any, userId: string): Promise<Result<any>> {
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

async function listEvents(config: Config, creds: UserCreds, input: any, userId: string): Promise<Result<any>> {
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

async function getEvent(config: Config, creds: UserCreds, input: any, userId: string): Promise<Result<any>> {
  if (!input?.eventId) return fail({ code: "MISCONFIGURED", message: "eventId required" });
  const r = await callCal(config, creds, eventPath(input), undefined, userId);
  if (!r.ok) return r;
  return ok(parseEvent(r.data));
}

async function createEvent(config: Config, creds: UserCreds, input: any, userId: string): Promise<Result<any>> {
  let composed;
  try { composed = composeEvent(input as ComposeInput); }
  catch (e: any) { return fail({ code: "MISCONFIGURED", message: e?.message ?? "invalid input" }); }
  const p = new URLSearchParams({ sendUpdates: input.sendUpdates ?? "all" });
  if (composed.conferenceDataVersion === 1) p.set("conferenceDataVersion", "1");
  const r = await callCal(config, creds, `${eventsPath(input)}?${p}`, jsonReq("POST", composed.body), userId);
  if (!r.ok) return r;
  return ok({ eventId: r.data.id, htmlLink: r.data.htmlLink, hangoutLink: r.data.hangoutLink ?? "" });
}

async function updateEvent(config: Config, creds: UserCreds, input: any, userId: string): Promise<Result<any>> {
  if (!input?.eventId || !input?.patch) return fail({ code: "MISCONFIGURED", message: "eventId and patch required" });
  const p = new URLSearchParams({ sendUpdates: input.sendUpdates ?? "all" });
  const r = await callCal(config, creds, `${eventPath(input)}?${p}`, jsonReq("PATCH", input.patch), userId);
  if (!r.ok) return r;
  return ok(parseEvent(r.data));
}

async function deleteEvent(config: Config, creds: UserCreds, input: any, userId: string): Promise<Result<any>> {
  if (!input?.eventId) return fail({ code: "MISCONFIGURED", message: "eventId required" });
  const p = new URLSearchParams({ sendUpdates: input.sendUpdates ?? "all" });
  const r = await callCal(config, creds, `${eventPath(input)}?${p}`, { method: "DELETE" }, userId);
  if (!r.ok) return r;
  return ok({ ok: true });
}

async function respondEvent(config: Config, creds: UserCreds, input: any, userId: string): Promise<Result<any>> {
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

async function quickAdd(config: Config, creds: UserCreds, input: any, userId: string): Promise<Result<any>> {
  if (!input?.text) return fail({ code: "MISCONFIGURED", message: "text required" });
  const p = new URLSearchParams({ text: input.text, sendUpdates: input.sendUpdates ?? "all" });
  const r = await callCal(config, creds, `${eventsPath(input)}/quickAdd?${p}`, { method: "POST" }, userId);
  if (!r.ok) return r;
  return ok({ eventId: r.data.id, htmlLink: r.data.htmlLink });
}

async function freebusy(config: Config, creds: UserCreds, input: any, userId: string): Promise<Result<any>> {
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

async function watchAction(config: Config, creds: UserCreds, input: any, userId: string): Promise<Result<any>> {
  if (!input?.address) return fail({ code: "MISCONFIGURED", message: "address required" });
  const body: any = { id: cryptoRandomId(), type: "web_hook", address: input.address };
  if (input.token) body.token = input.token;
  if (input.ttlSeconds) body.params = { ttl: String(input.ttlSeconds) };
  const r = await callCal(config, creds, `/calendars/${calId(input)}/events/watch`, jsonReq("POST", body), userId);
  if (!r.ok) return r;
  return ok({ channelId: r.data.id, resourceId: r.data.resourceId, expiration: r.data.expiration ? Number(r.data.expiration) : null });
}

async function updateCalendarVisibility(_config: Config, _creds: UserCreds, input: any, userId: string): Promise<Result<any>> {
  if (!input?.calendarExternalId || !input?.visibility) {
    return fail({ code: "MISCONFIGURED", message: "calendarExternalId and visibility required" });
  }
  if (input.visibility !== "share_everything" && input.visibility !== "metadata") {
    return fail({ code: "MISCONFIGURED", message: "visibility must be share_everything or metadata" });
  }
  const updated = await db`
    UPDATE google_calendar.calendars SET visibility = ${input.visibility}
    WHERE user_id = ${userId} AND external_id = ${input.calendarExternalId}
    RETURNING id
  `;
  if (!updated.length) return fail({ code: "NOT_FOUND", message: "calendar not found for this user" });
  return ok({ ok: true });
}

async function stopWatch(config: Config, creds: UserCreds, input: any, userId: string): Promise<Result<any>> {
  if (!input?.channelId || !input?.resourceId) return fail({ code: "MISCONFIGURED", message: "channelId and resourceId required" });
  const r = await callCal(config, creds, "/channels/stop", jsonReq("POST", { id: input.channelId, resourceId: input.resourceId }), userId);
  if (!r.ok) return r;
  return ok({});
}

async function syncConnect(config: Config, creds: UserCreds, _input: any, userId: string, token: string): Promise<Result<any>> {
  const cals = await listCalendars(config, creds, {}, userId);
  if (!cals.ok) return cals;

  const selected = cals.data.calendars.filter((c: any) => c.selected && !c.deleted);
  if (!selected.length) return fail({ code: "MISCONFIGURED", message: "no selected calendars" });
  const selectedIds = selected.map((c: any) => c.id);

  const calRows = selected.map((c: any) => ({
    external_id: c.id, user_id: userId, summary: c.summary, description: c.description,
    primary: c.primary, time_zone: c.timeZone, access_role: c.accessRole,
    background_color: c.backgroundColor, foreground_color: c.foregroundColor, selected: c.selected,
    visibility: "share_everything",
  }));
  await db`
    INSERT INTO google_calendar.calendars ${db(calRows)}
    ON CONFLICT (user_id, external_id) DO UPDATE SET
      summary = EXCLUDED.summary, description = EXCLUDED.description, "primary" = EXCLUDED."primary",
      time_zone = EXCLUDED.time_zone, access_role = EXCLUDED.access_role,
      background_color = EXCLUDED.background_color, foreground_color = EXCLUDED.foreground_color,
      selected = EXCLUDED.selected
  `;

  await db`UPDATE google_calendar.sync_cursors SET enabled = false WHERE user_id = ${userId} AND calendar_external_id != ALL(${selectedIds})`;

  const cursorRows = selected.map((c: any) => ({
    user_id: userId, calendar_external_id: c.id, status: "idle", enabled: true, throttle_count: 0,
  }));
  const upserted = await db`
    INSERT INTO google_calendar.sync_cursors ${db(cursorRows)}
    ON CONFLICT (user_id, calendar_external_id) DO UPDATE SET
      enabled = true, status = 'idle', throttle_count = 0, throttle_after = null
    RETURNING id
  `;
  const cursorIds = upserted.map((r: any) => r.id);

  await syncUserNow(userId, token);
  return ok({ cursor_ids: cursorIds, calendars: selected.map((c: any) => ({ id: c.id, summary: c.summary })) });
}

async function syncDisconnect(_config: Config, _creds: UserCreds, _input: any, userId: string, _token: string): Promise<Result<any>> {
  await db`UPDATE google_calendar.sync_cursors SET enabled = false WHERE user_id = ${userId}`;
  return ok({ ok: true });
}

async function syncNow(_config: Config, _creds: UserCreds, _input: any, userId: string, token: string): Promise<Result<any>> {
  const cursors = await db`SELECT id, status, throttle_after FROM google_calendar.sync_cursors WHERE user_id = ${userId} AND enabled = true`;
  if (!cursors.length) return fail({ code: "MISCONFIGURED", message: "no active sync" });
  const blocked = cursors.every((c: any) =>
    c.status === "syncing" ||
    (c.throttle_after && new Date(c.throttle_after).getTime() > Date.now()),
  );
  if (blocked) return ok({ triggered: false });
  await syncUserNow(userId, token);
  return ok({ triggered: true });
}

async function syncUserNow(userId: string, token: string) {
  await db`UPDATE google_calendar.sync_cursors SET status = 'syncing' WHERE user_id = ${userId} AND enabled = true AND status != 'needs_reauth'`;
  const cursors = await db`SELECT * FROM google_calendar.sync_cursors WHERE user_id = ${userId} AND enabled = true`;
  for (const sc of cursors) {
    if (sc.throttle_after && new Date(sc.throttle_after).getTime() > Date.now()) continue;
    try {
      await syncCursor(token, userId, sc);
      await db`UPDATE google_calendar.sync_cursors SET status = 'idle', last_synced_at = NOW(), throttle_count = 0, throttle_after = null WHERE id = ${sc.id}`;
    } catch (e: any) {
      log.error(`sync ${userId}/${sc.calendar_external_id}: ${e.message}`);
      await handleSyncError(sc, e);
    }
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

const actions: Record<string, (c: Config, u: UserCreds, i: any, uid: string, token: string) => Promise<Result<any>>> = {
  sync_connect: syncConnect, sync_disconnect: syncDisconnect, sync_now: syncNow,
  get_profile: getProfile, list_calendars: listCalendars,
  update_calendar_visibility: updateCalendarVisibility,
  list_events: listEvents, get_event: getEvent,
  create_event: createEvent, update_event: updateEvent, delete_event: deleteEvent,
  respond_event: respondEvent, quick_add: quickAdd, freebusy: freebusy,
  watch: watchAction, stop_watch: stopWatch,
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
  const res = await fetch(`${ctx.runtimeUrl}/api/v1/integrations/google_calendar/actions/${action}`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Authorization: `Bearer ${token}` },
    body: JSON.stringify(input),
  });
  if (!res.ok) {
    const text = await res.text().catch(() => "");
    throw new Error(`google_calendar/${action} -> ${res.status}: ${text.slice(0, 200)}`);
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

async function syncCursor(token: string, userId: string, sc: any) {
  let pageToken: string | undefined;
  let nextSyncToken: string | undefined;
  const calId = sc.calendar_external_id;

  while (true) {
    const args: any = {
      calendarId: calId,
      maxResults: 250,
      singleEvents: true,
      pageToken,
    };
    if (sc.sync_token) args.syncToken = sc.sync_token;
    else args.showDeleted = true;

    let data: any;
    try { data = await selfAction(token, "list_events", args); }
    catch (e: any) {
      if (e.code === "SYNC_CURSOR_ERROR") {
        await db`UPDATE google_calendar.sync_cursors SET sync_token = null WHERE id = ${sc.id}`;
        return syncCursor(token, userId, { ...sc, sync_token: null });
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
      const orphan = await db`
        SELECT event_id FROM google_calendar.channel_event_associations
        WHERE user_id = ${userId} AND calendar_external_id = ${calId} AND event_external_id = ANY(${cancelled})
      `;
      await db`
        DELETE FROM google_calendar.channel_event_associations
        WHERE user_id = ${userId} AND calendar_external_id = ${calId} AND event_external_id = ANY(${cancelled})
      `;
      const orphanIds = orphan.map((r: any) => r.event_id);
      if (orphanIds.length) {
        await db`
          DELETE FROM google_calendar.events
          WHERE id = ANY(${orphanIds})
            AND NOT EXISTS (
              SELECT 1 FROM google_calendar.channel_event_associations a WHERE a.event_id = google_calendar.events.id
            )
        `;
      }
    }

    for (const ev of upserts) await upsertEvent(userId, calId, ev);

    pageToken = data.nextPageToken ?? undefined;
    nextSyncToken = data.nextSyncToken ?? nextSyncToken;
    if (!pageToken) break;
  }

  if (nextSyncToken) {
    await db`UPDATE google_calendar.sync_cursors SET sync_token = ${nextSyncToken} WHERE id = ${sc.id}`;
  }
}

async function upsertEvent(userId: string, calId: string, e: ParsedEvent) {
  if (!e.externalId) return;
  const toIso = (ms: number | null) => ms ? new Date(ms).toISOString() : null;

  const upserted = await db`
    INSERT INTO google_calendar.events (
      external_id, ical_uid, title, description, location,
      starts_at, ends_at, is_full_day, time_zone,
      external_created_at, external_updated_at,
      recurring_event_external_id, recurrence,
      conference_solution, conference_link, html_link, organizer_address,
      transparency, visibility
    ) VALUES (
      ${e.externalId}, ${e.iCalUid || null}, ${e.title}, ${e.description}, ${e.location},
      ${toIso(e.startsAt)}, ${toIso(e.endsAt)}, ${e.isFullDay}, ${e.timeZone},
      ${toIso(e.externalCreatedAt)}, ${toIso(e.externalUpdatedAt)},
      ${e.recurringEventExternalId}, ${JSON.stringify(e.recurrence)},
      ${e.conferenceSolution}, ${e.conferenceLink}, ${e.htmlLink}, ${e.organizerAddress},
      ${e.transparency}, ${e.visibility}
    )
    ON CONFLICT (external_id) DO UPDATE SET
      ical_uid = EXCLUDED.ical_uid,
      title = EXCLUDED.title,
      description = EXCLUDED.description,
      location = EXCLUDED.location,
      starts_at = EXCLUDED.starts_at,
      ends_at = EXCLUDED.ends_at,
      is_full_day = EXCLUDED.is_full_day,
      time_zone = EXCLUDED.time_zone,
      external_created_at = EXCLUDED.external_created_at,
      external_updated_at = EXCLUDED.external_updated_at,
      recurring_event_external_id = EXCLUDED.recurring_event_external_id,
      recurrence = EXCLUDED.recurrence,
      conference_solution = EXCLUDED.conference_solution,
      conference_link = EXCLUDED.conference_link,
      html_link = EXCLUDED.html_link,
      organizer_address = EXCLUDED.organizer_address,
      transparency = EXCLUDED.transparency,
      visibility = EXCLUDED.visibility
    WHERE google_calendar.events.external_updated_at IS NULL
       OR EXCLUDED.external_updated_at IS NULL
       OR EXCLUDED.external_updated_at >= google_calendar.events.external_updated_at
    RETURNING id
  `;
  if (!upserted.length) return;
  const eventId = upserted[0].id;

  await db`
    INSERT INTO google_calendar.channel_event_associations
      (event_id, user_id, calendar_external_id, event_external_id, recurring_event_external_id)
    VALUES (${eventId}, ${userId}, ${calId}, ${e.externalId}, ${e.recurringEventExternalId})
    ON CONFLICT (user_id, calendar_external_id, event_external_id) DO NOTHING
  `;

  if (e.attendees.length) {
    const rows = e.attendees.map(a => ({
      event_id: eventId, address: a.address, display_name: a.displayName,
      response_status: a.responseStatus, is_organizer: a.isOrganizer, optional: a.optional,
    }));
    await db`
      INSERT INTO google_calendar.attendees ${db(rows)}
      ON CONFLICT (event_id, lower(address)) DO UPDATE SET
        display_name = EXCLUDED.display_name,
        response_status = EXCLUDED.response_status,
        is_organizer = EXCLUDED.is_organizer,
        optional = EXCLUDED.optional
    `;
    const keepAddrs = e.attendees.map(a => a.address.toLowerCase());
    await db`DELETE FROM google_calendar.attendees WHERE event_id = ${eventId} AND lower(address) != ALL(${keepAddrs})`;
  } else {
    await db`DELETE FROM google_calendar.attendees WHERE event_id = ${eventId}`;
  }

  const attachmentsWithFileId = e.attachments.filter(a => a.fileId);
  if (attachmentsWithFileId.length) {
    const rows = attachmentsWithFileId.map(a => ({
      event_id: eventId, file_id: a.fileId, file_url: a.fileUrl,
      title: a.title, mime_type: a.mimeType, icon_link: a.iconLink,
    }));
    await db`
      INSERT INTO google_calendar.event_attachments ${db(rows)}
      ON CONFLICT (event_id, file_id) WHERE file_id IS NOT NULL AND file_id <> '' DO UPDATE SET
        file_url = EXCLUDED.file_url,
        title = EXCLUDED.title,
        mime_type = EXCLUDED.mime_type,
        icon_link = EXCLUDED.icon_link
    `;
    const keepFileIds = attachmentsWithFileId.map(a => a.fileId);
    await db`DELETE FROM google_calendar.event_attachments WHERE event_id = ${eventId} AND file_id IS NOT NULL AND file_id <> '' AND file_id != ALL(${keepFileIds})`;
  } else {
    await db`DELETE FROM google_calendar.event_attachments WHERE event_id = ${eventId} AND file_id IS NOT NULL AND file_id <> ''`;
  }
}

async function handleSyncError(sc: any, err: any) {
  if (err.code === "INSUFFICIENT_PERMISSIONS") {
    await db`UPDATE google_calendar.sync_cursors SET status = 'needs_reauth' WHERE id = ${sc.id}`;
    return;
  }
  if (err.code === "SYNC_CURSOR_ERROR") {
    await db`UPDATE google_calendar.sync_cursors SET sync_token = null, status = 'idle' WHERE id = ${sc.id}`;
    return;
  }
  const count = (sc.throttle_count ?? 0) + 1;
  if (count >= MAX_THROTTLE) {
    await db`UPDATE google_calendar.sync_cursors SET status = 'failed_permanent', throttle_count = ${count} WHERE id = ${sc.id}`;
    return;
  }
  const wait = 60_000 * Math.pow(2, Math.min(count - 1, 5));
  const after = new Date(Math.max(Date.now() + wait, err.retryAfter ?? 0)).toISOString();
  await db`UPDATE google_calendar.sync_cursors SET status = 'failed_temporary', throttle_count = ${count}, throttle_after = ${after} WHERE id = ${sc.id}`;
}

