import { describe, expect, test } from 'bun:test';
import { plugins } from '../src/registry';
import { createRuntime } from '../src/runtime';
import { context } from '../src/http';
import { salesforceOrigin } from '../src/providers/salesforce';
import type { ObjectValue, Plugin } from '../src/types';

// Deterministic vendor HTTP fixtures. These tests do not qualify a live SaaS account.
const byId = (id: string) => plugins.find(p => p.id === id)!;
function harness(plugin: Plugin, replies: unknown[] = [{}]) {
  const calls: { url: URL; init: RequestInit }[] = [];
  const fetcher = (async (url: string | URL, init: RequestInit) => {
    calls.push({ url: new URL(String(url)), init });
    const reply = replies.shift();
    if (reply instanceof Error) throw reply;
    return reply instanceof Response ? reply : Response.json(reply ?? {});
  }) as unknown as typeof fetch;
  const rpc = createRuntime(plugin, fetcher);
  const connectionId = crypto.randomUUID();
  return { calls, rpc, run: (action: string, input: ObjectValue = {}) => rpc.__integration({ action, input, connectionId, userCredentials: { accessToken: 'fixture-token', instanceUrl: 'https://fixture.my.salesforce.com' } }) };
}

describe('vendor pagination contracts', () => {
  const cases: [string, string, ObjectValue, ObjectValue, string, string, string][] = [
    ['notion', 'list_sources', {}, { results: [{ id: 'ds', title: [{ plain_text: 'Clients' }] }], next_cursor: 'next' }, '/v1/search', 'POST', 'next'],
    ['stripe', 'list_customers', { cursor: 'cus_before' }, { data: [{ id: 'cus_last' }], has_more: true }, '/v1/customers', 'GET', 'cus_last'],
    ['hubspot', 'list_contacts', {}, { results: [{ id: '123' }], paging: { next: { after: 42 } } }, '/crm/v3/objects/contacts', 'GET', '42'],
    ['salesforce', 'query_records', { query: 'SELECT Id FROM Contact' }, { records: [{ Id: '123' }], done: false, nextRecordsUrl: '/services/data/v61.0/query/next' }, '/services/data/v61.0/query', 'GET', '/services/data/v61.0/query/next'],
    ['airtable', 'list_records', { baseId: 'app123', tableId: 'tbl123' }, { records: [{ id: 'rec123' }], offset: 'next' }, '/v0/app123/tbl123', 'GET', 'next'],
    ['gmail', 'search_messages', {}, { messages: [{ id: '123' }], nextPageToken: 'next' }, '/gmail/v1/users/me/messages', 'GET', 'next'],
    ['google_calendar', 'list_events', { calendarId: 'a@example.com' }, { items: [{ id: '123' }], nextPageToken: 'next' }, '/calendar/v3/calendars/a%40example.com/events', 'GET', 'next'],
    ['google_sheets', 'list_spreadsheets', {}, { files: [{ id: '123' }], nextPageToken: 'next' }, '/drive/v3/files', 'GET', 'next'],
    ['google_drive', 'list_files', {}, { files: [{ id: '123' }], nextPageToken: 'next' }, '/drive/v3/files', 'GET', 'next'],
    ['slack', 'list_channels', {}, { ok: true, channels: [{ id: 'C123' }], response_metadata: { next_cursor: 'next' } }, '/api/conversations.list', 'GET', 'next'],
  ];
  test.each(cases)('%s returns explicit continuation without silently fetching extra pages', async (id, action, input, response, path, method, cursor) => {
    const h = harness(byId(id), [response]);
    const result = await h.run(action, input);
    expect(result).toMatchObject({ ok: true, data: { nextCursor: cursor } });
    expect(h.calls).toHaveLength(1);
    expect(h.calls[0].url.pathname).toBe(path);
    expect(h.calls[0].init.method).toBe(method);
    expect(new Headers(h.calls[0].init.headers).get('authorization')).toBe('Bearer fixture-token');
    expect(h.calls[0].init.redirect).toBe('error');
  });
});

test('Slack channel listing sends pagination as URL parameters, not an ignored JSON body', async () => {
  const h = harness(byId('slack'), [{ ok: true, channels: [{ id: 'C123' }], response_metadata: { next_cursor: 'after' } }]);
  expect(await h.run('list_channels', { limit: 1, cursor: 'before+/=' })).toMatchObject({ ok: true, data: { nextCursor: 'after' } });
  expect(h.calls[0].init.method).toBe('GET');
  expect(h.calls[0].init.body).toBeUndefined();
  expect(Object.fromEntries(h.calls[0].url.searchParams)).toEqual({ limit: '1', cursor: 'before+/=', types: 'public_channel,private_channel', exclude_archived: 'true' });
  for (const error of ['invalid_auth', 'token_revoked', 'token_expired', 'missing_scope']) {
    const rejected = harness(byId('slack'), [{ ok: false, error }]);
    expect(await rejected.run('list_channels'), error).toMatchObject({ ok: false, error: { code: error === 'missing_scope' ? 'PROVIDER_REJECTED' : 'unauthorized', outcome: 'rejected' } });
  }
});

test('Notion writes use data-source parents and reject computed properties before creating', async () => {
  const schema = { properties: { Name: { type: 'title' }, Total: { type: 'formula' } } };
  const h = harness(byId('notion'), [schema, { id: 'page1' }]);
  const properties = { Name: { title: [{ text: { content: 'Client' } }] } };
  expect(await h.run('create_entry', { dataSourceId: 'source1', properties })).toMatchObject({ ok: true, data: { id: 'page1' } });
  expect(JSON.parse(String(h.calls[1].init.body))).toEqual({ parent: { type: 'data_source_id', data_source_id: 'source1' }, properties });
  expect(new Headers(h.calls[1].init.headers).get('Notion-Version')).toBe('2025-09-03');
  const invalid = harness(byId('notion'), [schema]);
  expect(await invalid.run('create_entry', { dataSourceId: 'source1', properties: { Total: { formula: 1 } } })).toMatchObject({ ok: false, error: { code: 'INVALID_INPUT' } });
  expect(invalid.calls).toHaveLength(1);
});

test('Stripe preserves explicit idempotency and nested metadata in form encoding', async () => {
  const h = harness(byId('stripe'), [{ id: 'cus_new' }]);
  expect(await h.run('create_customer', { name: 'A & B', metadata: { app: 'crm' }, idempotencyKey: 'intent-1' })).toMatchObject({ ok: true });
  expect(new Headers(h.calls[0].init.headers).get('Idempotency-Key')).toBe('intent-1');
  expect(new URLSearchParams(String(h.calls[0].init.body)).get('metadata[app]')).toBe('crm');
  const missing = harness(byId('stripe'));
  expect(await missing.run('create_customer', { name: 'A' })).toMatchObject({ ok: false, error: { code: 'INVALID_INPUT' } });
  expect(missing.calls).toHaveLength(0);
});

test.each([
  ['hubspot', 'update_contact', { contactId: '123', properties: { firstname: 'Jane' } }, '/crm/v3/objects/contacts/123', { properties: { firstname: 'Jane' } }],
  ['salesforce', 'update_record', { objectType: 'Contact', recordId: '123', fields: { LastName: 'Doe' } }, '/services/data/v61.0/sobjects/Contact/123', { LastName: 'Doe' }],
  ['airtable', 'update_record', { baseId: 'app123', tableId: 'tbl123', recordId: 'rec123', fields: { Name: 'Jane' } }, '/v0/app123/tbl123/rec123', { fields: { Name: 'Jane' }, typecast: false }],
] as [string, string, ObjectValue, string, ObjectValue][])('%s updates the existing ID instead of creating a duplicate', async (id, action, input, path, body) => {
  const h = harness(byId(id));
  expect(await h.run(action, input)).toMatchObject({ ok: true });
  expect(h.calls).toHaveLength(1);
  expect(h.calls[0].init.method).toBe('PATCH');
  expect(h.calls[0].url.pathname).toBe(path);
  expect(JSON.parse(String(h.calls[0].init.body))).toEqual(body);
});

test('Sheets writes literal RAW values and escapes A1 ranges', async () => {
  const h = harness(byId('google_sheets'));
  expect(await h.run('append_rows', { spreadsheetId: 'sheet1', range: "'My Sheet'!A1:B2", values: [['=1+1', 2]] })).toMatchObject({ ok: true });
  expect(h.calls[0].url.searchParams.get('valueInputOption')).toBe('RAW');
  expect(h.calls[0].url.pathname).toContain('My%20Sheet');
  expect(JSON.parse(String(h.calls[0].init.body)).values).toEqual([['=1+1', 2]]);
});

test('Calendar validates dates before sending and transmits the stable creation ID', async () => {
  const input = { calendarId: 'primary', eventId: 'abcde12345', summary: 'Test', start: { date: '2026-10-14' }, end: { date: '2026-10-15' } };
  const h = harness(byId('google_calendar'));
  expect(await h.run('create_event', input)).toMatchObject({ ok: true });
  expect(JSON.parse(String(h.calls[0].init.body)).id).toBe(input.eventId);
  for (const date of ['garbage', '2026-10-13']) {
    const bad = harness(byId('google_calendar'));
    expect(await bad.run('create_event', { ...input, end: { date } })).toMatchObject({ ok: false, error: { code: 'INVALID_INPUT' } });
    expect(bad.calls).toHaveLength(0);
  }
});

test('Gmail encodes Unicode safely and rejects header injection', async () => {
  const h = harness(byId('gmail'));
  expect(await h.run('create_draft', { to: ['a@example.com'], subject: 'é'.repeat(90), text: 'Bonjour é' })).toMatchObject({ ok: true });
  const raw = JSON.parse(String(h.calls[0].init.body)).message.raw;
  const mime = Buffer.from(raw, 'base64url').toString('utf8');
  expect(mime.match(/=\?UTF-8\?B\?[^?]+\?=/g)?.every(word => word.length <= 75)).toBe(true);
  expect(mime).toContain(Buffer.from('Bonjour é').toString('base64'));
  expect(await h.run('send_message', { to: ['a@example.com'], subject: 'a\r\nBcc: evil@example.com', text: '' })).toMatchObject({ ok: false, error: { code: 'INVALID_INPUT' } });
  expect(h.calls).toHaveLength(1);
});

test('Drive multipart upload preserves binary bytes and rejects MIME injection', async () => {
  const h = harness(byId('google_drive'));
  const input = { name: 'file.bin', mimeType: 'application/octet-stream', contentBase64: 'AP8B' };
  expect(await h.run('upload_file', input)).toMatchObject({ ok: true });
  const bytes = Buffer.from(await (h.calls[0].init.body as Blob).arrayBuffer());
  expect(bytes.includes(Buffer.from([0, 255, 1]))).toBe(true);
  expect(await h.run('upload_file', { ...input, mimeType: 'text/plain\r\nX: y' })).toMatchObject({ ok: false });
  expect(h.calls).toHaveLength(1);
});

test('Slack treats HTTP 200 ok:false as a failure and does not retry sends', async () => {
  const h = harness(byId('slack'), [{ ok: false, error: 'token_revoked' }]);
  expect(await h.run('send_message', { channelId: 'C123', text: 'Hello' })).toMatchObject({ ok: false, error: { code: 'unauthorized' } });
  expect(h.calls).toHaveLength(1);
});

test.each([401, 403, 429, 503])('HTTP %i has a safe error envelope, no raw provider body and no automatic write retry', async status => {
  const h = harness(byId('stripe'), [new Response('secret-token and private customer details', { status, headers: { 'Retry-After': '12' } })]);
  const result = await h.run('create_customer', { name: 'Jane', idempotencyKey: 'intent-2' });
  expect(result).toMatchObject({ ok: false, error: { status, outcome: status >= 500 ? 'unknown' : 'rejected' } });
  expect(JSON.stringify(result)).not.toContain('secret-token');
  expect(h.calls).toHaveLength(1);
  if (status === 429) { expect(await h.run('list_customers')).toMatchObject({ ok: false, error: { retryAfter: 12 } }); expect(h.calls).toHaveLength(1); }
});

test('Network failure after sending marks writes uncertain', async () => {
  const h = harness(byId('gmail'), [new Error('secret-token')]);
  expect(await h.run('send_message', { to: ['a@example.com'], subject: 'Hi', text: '' })).toMatchObject({ ok: false, error: { code: 'TRANSPORT_ERROR', outcome: 'unknown' } });
  expect(h.calls).toHaveLength(1);
});

test('Paths, services and dynamic Salesforce hosts cannot redirect credentials', async () => {
  let calls = 0;
  const ctx = context(byId('stripe'), { accessToken: 'fixture' }, crypto.randomUUID(), (async () => { calls++; return Response.json({}); }) as unknown as typeof fetch);
  for (const path of ['https://evil.example/x', '//evil.example/x', '/\\evil.example/x']) await expect(ctx.request({ path })).rejects.toThrow();
  await expect(ctx.request({ path: '/', service: 'evil' })).rejects.toThrow();
  await expect(ctx.request({ path: '/', headers: { Authorization: 'injected' } })).rejects.toThrow();
  for (const instanceUrl of ['http://x.salesforce.com', 'https://salesforce.com.evil.example', 'https://x.salesforce.com@evil.example', 'https://127.0.0.1', 'https://x.salesforce.com/path']) expect(() => salesforceOrigin({ accessToken: 'x', instanceUrl })).toThrow();
  expect(calls).toBe(0);
});

test('Input validation and account verification fail closed across the catalog', async () => {
  for (const plugin of plugins) {
    const h = harness(plugin, [{}]);
    expect(await h.run('verify_connection', { accessToken: 'override' })).toMatchObject({ ok: false, error: { code: 'INVALID_INPUT' } });
    expect(await h.run('not_registered')).toMatchObject({ ok: false, error: { code: 'UNKNOWN_ACTION' } });
    expect(h.calls).toHaveLength(0);
    if (plugin.tokenAuth) await expect(h.rpc.__auth_credentials({ userId: crypto.randomUUID(), credentials: { accessToken: 'fixture' } })).rejects.toThrow();
  }
});

test('Response size is bounded and partial successful writes remain uncertain', async () => {
  const h = harness(byId('stripe'), [new Response('{}', { headers: { 'Content-Length': '10485761' } })]);
  expect(await h.run('create_customer', { name: 'Jane', idempotencyKey: 'intent-3' })).toMatchObject({ ok: false, error: { code: 'RESPONSE_TOO_LARGE', outcome: 'unknown' } });
});

test('Malformed vendor pagination is not reported as a successful empty data source', async () => {
  const h = harness(byId('stripe'), [{ has_more: false }]);
  expect(await h.run('list_customers')).toMatchObject({ ok: false, error: { code: 'INVALID_RESPONSE' } });
});

test.each([true, false])('Drive trash toggle %s updates only the selected existing file', async trashed => {
  const h = harness(byId('google_drive'));
  expect(await h.run('update_file', { fileId: 'test-file', trashed })).toMatchObject({ ok: true });
  expect(h.calls).toHaveLength(1);
  expect(h.calls[0].init.method).toBe('PATCH');
  expect(h.calls[0].url.pathname).toBe('/drive/v3/files/test-file');
  expect(h.calls[0].url.searchParams.get('supportsAllDrives')).toBe('true');
  expect(JSON.parse(String(h.calls[0].init.body))).toEqual({ trashed });
});
test('Drive metadata update rejects empty changes and unsafe file identifiers without HTTP', async () => {
  for (const input of [{ fileId: 'test' }, { fileId: '../other', name: 'x' }, { fileId: '', trashed: true }]) {
    const h = harness(byId('google_drive'));
    expect(await h.run('update_file', input)).toMatchObject({ ok: false });
    expect(h.calls).toHaveLength(0);
  }
});
