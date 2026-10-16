import { expect, test } from 'bun:test';
import notion from '../src/providers/notion';
import hubspot from '../src/providers/hubspot';
import salesforce from '../src/providers/salesforce';
import airtable from '../src/providers/airtable';
import { createRuntime } from '../src/runtime';
import type { ObjectValue, Plugin } from '../src/types';

function harness(plugin: Plugin, replies: unknown[]) {
  const calls: { url: URL; init: RequestInit }[] = [];
  const rpc = createRuntime(plugin, (async (url, init) => {
    calls.push({ url: new URL(String(url)), init: init! });
    return Response.json(replies.shift());
  }) as typeof fetch);
  const connectionId = crypto.randomUUID();
  return { calls, rpc, run: (action: string, input: ObjectValue = {}) => rpc.__integration({ action, input, connectionId, userCredentials: { accessToken: 'fixture', instanceUrl: 'https://fixture.my.salesforce.com' } }) };
}

test('Salesforce read queries reject locking and metadata writes before sending', async () => {
  for (const clause of ['FOR UPDATE', 'for view', 'FOR\nREFERENCE', 'UPDATE TRACKING', 'UPDATE VIEWSTAT']) {
    const h = harness(salesforce, []);
    expect(await h.run('query_records', { query: `SELECT Id FROM Contact ${clause}` })).toMatchObject({ ok: false, error: { code: 'INVALID_INPUT' } });
    expect(h.calls, clause).toHaveLength(0);
  }
  const h = harness(salesforce, [{ records: [], done: true }]);
  expect(await h.run('query_records', { query: "SELECT Id FROM Contact WHERE LastName = 'FOR UPDATE FROM Account'" })).toMatchObject({ ok: true });
});

test('Salesforce rejects missing or unsafe continuation instead of silently truncating results', async () => {
  for (const response of [{ records: [] }, { records: [], done: false }, { records: [], done: false, nextRecordsUrl: 'https://evil.example/query' }]) {
    const h = harness(salesforce, [response]);
    expect(await h.run('query_records', { query: 'SELECT Id FROM Contact' })).toMatchObject({ ok: false, error: { code: 'INVALID_RESPONSE' } });
  }
  const h = harness(salesforce, [{ records: [{ Id: 'record' }], done: true }]);
  expect(await h.run('query_records', { cursor: '/services/data/v61.0/query/01gABC-2000' })).toMatchObject({ ok: true, data: { nextCursor: null } });
  expect(h.calls[0].url.pathname).toBe('/services/data/v61.0/query/01gABC-2000');
});

test('HubSpot search sends numeric offsets and rejects invalid or exhausted search cursors', async () => {
  for (const cursor of ['-1', 'x', '1.5', '10000', '9007199254740993']) {
    const h = harness(hubspot, []);
    expect(await h.run('search_contacts', { cursor })).toMatchObject({ ok: false, error: { code: 'INVALID_INPUT' } });
    expect(h.calls, cursor).toHaveLength(0);
  }
  const h = harness(hubspot, [{ results: [], paging: { next: { after: '100' } } }]);
  expect(await h.run('search_contacts', { cursor: '50' })).toMatchObject({ ok: true, data: { nextCursor: '100' } });
  expect(JSON.parse(String(h.calls[0].init.body)).after).toBe(50);
});

test('HubSpot authorization uses v3 form exchange and retains the verified portal identity', async () => {
  const h = harness(hubspot, [{ access_token: 'access', refresh_token: 'refresh', expires_in: 1800 }, { portalId: 123 }]);
  expect(await h.rpc.__auth_callback({ userId: crypto.randomUUID(), config: { clientId: 'client', clientSecret: 'secret' }, query: { code: 'code' }, callbackUrl: 'https://example.com/callback' })).toMatchObject({ credentials: { accessToken: 'access', refreshToken: 'refresh' }, label: 'HubSpot 123 [123]' });
  expect(h.calls[0].url.href).toBe('https://api.hubspot.com/oauth/v3/token');
  expect(new URLSearchParams(String(h.calls[0].init.body)).get('client_secret')).toBe('secret');
  expect(h.calls[0].init.redirect).toBe('error');
});

test('Notion paginated property reads preserve encoded IDs and explicit continuation', async () => {
  const h = harness(notion, [{ object: 'list', results: [{ type: 'relation', relation: { id: 'related' } }], next_cursor: 'next', has_more: true, property_item: { type: 'relation' } }]);
  expect(await h.run('get_entry_property', { pageId: 'page', propertyId: 'a%3Ab', cursor: 'before', limit: 25 })).toMatchObject({ ok: true, data: { nextCursor: 'next', items: [{ relation: { id: 'related' } }] } });
  expect(h.calls[0].url.pathname).toBe('/v1/pages/page/properties/a%3Ab');
  expect(h.calls[0].url.searchParams.get('start_cursor')).toBe('before');
  expect(h.calls).toHaveLength(1);
});

test('Notion single property values share the pagination envelope and unsafe IDs never send', async () => {
  const h = harness(notion, [{ object: 'property_item', type: 'number', number: 0 }]);
  expect(await h.run('get_entry_property', { pageId: 'page', propertyId: 'value' })).toMatchObject({ ok: true, data: { items: [{ number: 0 }], nextCursor: null } });
  for (const propertyId of ['%', '..', '%2E%2E', '%2Fother', 'a\\b']) {
    const invalid = harness(notion, []);
    expect(await invalid.run('get_entry_property', { pageId: 'page', propertyId })).toMatchObject({ ok: false, error: { code: 'INVALID_INPUT' } });
    expect(invalid.calls, propertyId).toHaveLength(0);
  }
});

test('CRM verification fails closed on malformed provider identities', async () => {
  for (const [plugin, response] of [[notion, { id: {} }], [airtable, { id: 123 }], [hubspot, { portalId: {} }], [salesforce, { organization_id: {}, user_id: 'user' }]] as [Plugin, ObjectValue][]) {
    const h = harness(plugin, [response]);
    expect(await h.run('verify_connection'), plugin.id).toMatchObject({ ok: false });
  }
});

test('Airtable forwards opaque continuation without silently fetching more records', async () => {
  const h = harness(airtable, [{ records: [], offset: 'next/token' }]);
  expect(await h.run('list_records', { baseId: 'app123', tableId: 'tbl123', cursor: 'opaque/token' })).toMatchObject({ ok: true, data: { nextCursor: 'next/token' } });
  expect(h.calls[0].url.searchParams.get('offset')).toBe('opaque/token');
  expect(h.calls).toHaveLength(1);
});

test('Notion property responses cannot silently lose continuation or return invalid objects', async () => {
  for (const response of [{}, { object: 'list', results: [], has_more: true }]) {
    const h = harness(notion, [response]);
    expect(await h.run('get_entry_property', { pageId: 'page', propertyId: 'value' })).toMatchObject({ ok: false, error: { code: 'INVALID_RESPONSE' } });
  }
});

test('Salesforce rejections name the error code and fields without echoing submitted values', async () => {
  const rpc = createRuntime(salesforce, (async () => Response.json([{ message: 'private-value is invalid', errorCode: 'REQUIRED_FIELD_MISSING', fields: ['LastName'] }], { status: 400 })) as unknown as typeof fetch);
  const result = await rpc.__integration({ action: 'create_record', input: { objectType: 'Contact', fields: { FirstName: 'private-value' } }, connectionId: crypto.randomUUID(), userCredentials: { accessToken: 'fixture', instanceUrl: 'https://fixture.my.salesforce.com' } });
  expect(result).toMatchObject({ ok: false, error: { code: 'PROVIDER_REJECTED', message: 'Salesforce returned HTTP 400 [REQUIRED_FIELD_MISSING LastName]' } });
  expect(JSON.stringify(result)).not.toContain('private-');
  const unknown = createRuntime(salesforce, (async () => Response.json([{ message: 'private', errorCode: 'PRIVATE_CODE', fields: ['LastName'] }], { status: 400 })) as unknown as typeof fetch);
  expect(await unknown.__integration({ action: 'create_record', input: { objectType: 'Contact', fields: { LastName: 'x' } }, connectionId: crypto.randomUUID(), userCredentials: { accessToken: 'fixture', instanceUrl: 'https://fixture.my.salesforce.com' } })).toMatchObject({ error: { message: 'Salesforce returned HTTP 400' } });
});
