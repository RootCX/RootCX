import { provider, googleOAuth, googleServices, pick } from './shared';
import { action, array, id, object, pagination, page, segment, string } from '../schema';
import { IntegrationError } from '../http';

const calendarId = string('Calendar ID, or primary');
const time = { ...object({ date: string('YYYY-MM-DD', { pattern: '^\\d{4}-\\d{2}-\\d{2}$' }), dateTime: string('RFC3339 timestamp with explicit UTC offset'), timeZone: string() }), oneOf: [{ required: ['date'], not: { required: ['dateTime'] } }, { required: ['dateTime'], not: { required: ['date'] } }] };
const event = { summary: string(), description: string(), location: string(), start: time, end: time, attendees: array(object({ email: string() }, ['email']), 100) };
const fields = Object.keys(event);
function validateTimestamp(value: string, dateOnly = false) {
  const pattern = dateOnly ? /^\d{4}-\d{2}-\d{2}$/ : /^\d{4}-\d{2}-\d{2}T(?:[01]\d|2[0-3]):[0-5]\d:[0-5]\d(?:\.\d+)?(?:Z|[+-](?:[01]\d|2[0-3]):[0-5]\d)$/i;
  const date = value.slice(0, 10);
  const parsed = Date.parse(value);
  if (!pattern.test(value) || !Number.isFinite(parsed) || new Date(`${date}T00:00:00Z`).toISOString().slice(0, 10) !== date) throw new IntegrationError('INVALID_INPUT', 'Use a valid calendar date or RFC3339 timestamp with an explicit UTC offset');
}
function body(input: Record<string, any>) {
  for (const point of [input.start, input.end]) if (point) validateTimestamp(point.date ?? point.dateTime, Boolean(point.date));
  if (input.start && input.end && (Boolean(input.start.date) !== Boolean(input.end.date) || Date.parse(input.start.date ?? input.start.dateTime) >= Date.parse(input.end.date ?? input.end.dateTime))) throw new IntegrationError('INVALID_INPUT', 'Event end must follow start using matching date types');
  return pick(input, fields);
}
export default provider('google_calendar', 'Google Calendar', 'https://www.googleapis.com', {
  list_calendars: action('List calendars', 'Discover accessible calendars', 'read', pagination, [], async (ctx, input) => {
    const result = await ctx.request({ path: '/calendar/v3/users/me/calendarList', query: { maxResults: input.limit ?? 50, pageToken: input.cursor } });
    return page(result.items ?? [], result.nextPageToken);
  }),
  list_events: action('List events', 'Read a page of events in a calendar', 'read', { calendarId, ...pagination, timeMin: string(), timeMax: string(), query: string() }, ['calendarId'], async (ctx, input) => {
    for (const bound of [input.timeMin, input.timeMax]) if (bound) validateTimestamp(bound);
    if (input.timeMin && input.timeMax && Date.parse(input.timeMin) >= Date.parse(input.timeMax)) throw new IntegrationError('INVALID_INPUT', 'timeMax must follow timeMin');
    const result = await ctx.request({ path: `/calendar/v3/calendars/${segment(input.calendarId)}/events`, query: { maxResults: input.limit ?? 50, pageToken: input.cursor, timeMin: input.timeMin, timeMax: input.timeMax, q: input.query } });
    return page(result.items ?? [], result.nextPageToken);
  }),
  get_event: action('Get event', 'Read an existing event', 'read', { calendarId, eventId: id() }, ['calendarId', 'eventId'], (ctx, input) => ctx.request({ path: `/calendar/v3/calendars/${segment(input.calendarId)}/events/${segment(input.eventId)}` })),
  create_event: action('Create event', 'Create an event using a stable client-generated base32hex ID; reconcile conflicts', 'write', { calendarId, eventId: string('Stable ID: lowercase a-v and digits 0-9', { pattern: '^[a-v0-9]{5,1024}$', maxLength: 1024 }), ...event }, ['calendarId', 'eventId', 'summary', 'start', 'end'], (ctx, input) => ctx.request({ path: `/calendar/v3/calendars/${segment(input.calendarId)}/events`, method: 'POST', json: { ...body(input), id: input.eventId } })),
  update_event: action('Update event', 'Patch the selected event without replacing other fields', 'write', { calendarId, eventId: id(), ...event }, ['calendarId', 'eventId'], (ctx, input) => ctx.request({ path: `/calendar/v3/calendars/${segment(input.calendarId)}/events/${segment(input.eventId)}`, method: 'PATCH', json: body(input) })),
}, { services: googleServices, oauth: googleOAuth(['https://www.googleapis.com/auth/calendar']), discovery: { action: 'list_calendars', itemsPath: 'items', idPath: 'id', labelPath: 'summary' } });
