import { expect, test } from 'bun:test';
import { createRuntime } from '../src/runtime';
import gmail from '../src/providers/gmail';
import calendar from '../src/providers/google-calendar';
import slack from '../src/providers/slack';
import stripe from '../src/providers/stripe';
import type { ObjectValue, Plugin } from '../src/types';

// Fixtures verify wire contracts, not live supplier qualification.
function harness(plugin: Plugin, response: ObjectValue = {}) {
  const calls: RequestInit[] = [];
  const fetcher = (async (_url: unknown, init: RequestInit) => {
    calls.push(init);
    return Response.json(response);
  }) as typeof fetch;
  const rpc = createRuntime(plugin, fetcher);
  return { calls, run: (action: string, input: ObjectValue) => rpc.__integration({ action, input, connectionId: crypto.randomUUID(), userCredentials: { accessToken: 'fixture' } }) };
}

test('Gmail folds long recipient lists into valid MIME headers', async () => {
  const h = harness(gmail);
  const to = Array.from({ length: 50 }, (_, i) => `recipient${i}@example.com`);
  expect(await h.run('create_draft', { to, subject: 'Bonjour 🥐', text: 'Hello' })).toMatchObject({ ok: true });
  const wire = Buffer.from(JSON.parse(String(h.calls[0].body)).message.raw, 'base64url').toString();
  expect(wire.split('\r\n').every(line => Buffer.byteLength(line) <= 998)).toBe(true);
  expect(wire.split('\r\nSubject:')[0].replace(/\r\n /g, ' ')).toBe(`To: ${to.join(', ')}`);
  for (const address of ['a\r\nBcc: victim@example.com', `${'a'.repeat(250)}@example.com`]) {
    const bad = harness(gmail);
    expect(await bad.run('send_message', { to: [address], subject: 'Test', text: '' })).toMatchObject({ ok: false, error: { code: 'INVALID_INPUT' } });
    expect(bad.calls, address).toHaveLength(0);
  }
});

test('Calendar rejects impossible dates and ambiguous timestamps before writing', async () => {
  for (const start of [{ date: '2026-02-30' }, { dateTime: '2026-10-15' }, { dateTime: '2026-10-15T10:00:00' }, { dateTime: '2026-02-30T10:00:00Z' }, { dateTime: '2026-10-15T24:00:00Z' }]) {
    const h = harness(calendar);
    expect(await h.run('update_event', { calendarId: 'primary', eventId: 'event1', start }), JSON.stringify(start)).toMatchObject({ ok: false, error: { code: 'INVALID_INPUT' } });
    expect(h.calls).toHaveLength(0);
  }
  const h = harness(calendar);
  expect(await h.run('create_event', { calendarId: 'primary', eventId: 'abcde123', summary: 'Call', start: { dateTime: '2026-10-15T10:00:00+02:00' }, end: { dateTime: '2026-10-15T09:00:00Z' } })).toMatchObject({ ok: true });
});

test('Calendar refuses reversed or malformed read time windows', async () => {
  for (const bounds of [{ timeMin: 'yesterday' }, { timeMin: '2026-02-30T00:00:00Z' }, { timeMin: '2026-10-16T00:00:00Z', timeMax: '2026-10-15T00:00:00Z' }]) {
    const h = harness(calendar);
    expect(await h.run('list_events', { calendarId: 'primary', ...bounds }), JSON.stringify(bounds)).toMatchObject({ ok: false, error: { code: 'INVALID_INPUT' } });
    expect(h.calls).toHaveLength(0);
  }
});

test('Slack history respects commercial app page limits and continuation', async () => {
  const h = harness(slack, { ok: true, messages: [], response_metadata: { next_cursor: 'next' } });
  expect(await h.run('get_history', { channelId: 'C123' })).toMatchObject({ ok: true, data: { nextCursor: 'next' } });
  expect(JSON.parse(String(h.calls[0].body)).limit).toBe(15);
  const bad = harness(slack);
  expect(await bad.run('get_history', { channelId: 'C123', limit: 16 })).toMatchObject({ ok: false, error: { code: 'INVALID_INPUT' } });
  expect(bad.calls).toHaveLength(0);
});

test('Stripe rejects invalid metadata before constructing form keys', async () => {
  for (const metadata of [{ 'x][y': 'value' }, { ['x'.repeat(41)]: 'value' }, { key: 'x'.repeat(501) }, { key: { nested: 'value' } }, { key: null }, Object.fromEntries(Array.from({ length: 51 }, (_, i) => [`k${i}`, 'v']))]) {
    const h = harness(stripe);
    expect(await h.run('update_customer', { customerId: 'cus_1', metadata }), JSON.stringify(metadata)).toMatchObject({ ok: false, error: { code: 'INVALID_INPUT' } });
    expect(h.calls).toHaveLength(0);
  }
  const h = harness(stripe);
  expect(await h.run('update_customer', { customerId: 'cus_1', metadata: { ['x'.repeat(40)]: 'v'.repeat(500), remove: '' } })).toMatchObject({ ok: true });
  expect(new URLSearchParams(String(h.calls[0].body)).get('metadata[remove]')).toBe('');
});

test('Slack internal failures can be partially successful writes but never uncertain reads', async () => {
  for (const error of ['internal_error', 'fatal_error', undefined]) {
    const send = harness(slack, { ok: false, error });
    expect(await send.run('send_message', { channelId: 'C123', text: 'Test' }), String(error)).toMatchObject({ ok: false, error: { code: 'WRITE_UNCERTAIN', outcome: 'unknown' } });
    expect(send.calls).toHaveLength(1);
    const read = harness(slack, { ok: false, error });
    expect(await read.run('get_history', { channelId: 'C123' })).toMatchObject({ ok: false, error: { outcome: 'rejected' } });
  }
  const rejected = harness(slack, { ok: false, error: 'request_timeout' });
  expect(await rejected.run('send_message', { channelId: 'C123', text: 'Test' })).toMatchObject({ ok: false, error: { outcome: 'rejected' } });
});
