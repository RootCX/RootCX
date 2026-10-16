import { expect, test } from 'bun:test';
import hubspot from '../src/providers/hubspot';
import { createRuntime } from '../src/runtime';
import type { ObjectValue } from '../src/types';

const identity = { id: '149556624:42', label: 'HubSpot user 42' };
const userCredentials = { accessToken: 'fixture', hubspotUserLevel: true, hubspotUserIdentity: identity };
function harness(reply: unknown = {}, status = 200) {
  const calls: { url: URL; init: RequestInit }[] = [];
  const rpc = createRuntime(hubspot, (async (url, init) => {
    calls.push({ url: new URL(String(url)), init: init! });
    return Response.json(reply, { status });
  }) as typeof fetch);
  return { calls, run: (action: string, input: ObjectValue = {}, credentials: ObjectValue = userCredentials) => rpc.__integration({ action, input, connectionId: crypto.randomUUID(), userCredentials: credentials }) };
}

test('HubSpot user-level contact actions use supported routes while preserving request semantics', async () => {
  for (const [action, input, suffix, method] of [
    ['list_contacts', { limit: 2, cursor: 'next' }, '', 'GET'],
    ['search_contacts', { query: 'fixture', cursor: '50' }, '/search', 'POST'],
    ['get_contact', { contactId: '123' }, '/123', 'GET'],
    ['create_contact', { properties: { firstname: 'Fixture' } }, '', 'POST'],
    ['update_contact', { contactId: '123', properties: { firstname: 'Updated' } }, '/123', 'PATCH'],
  ] as const) {
    const h = harness({ results: [], id: '123' });
    expect(await h.run(action, input), action).toMatchObject({ ok: true });
    expect(h.calls[0].url.pathname, action).toBe(`/crm/objects/2026-09/contacts${suffix}`);
    expect(h.calls[0].init.method, action).toBe(method);
    if (action === 'list_contacts') expect(h.calls[0].url.searchParams.get('after')).toBe('next');
    if (action === 'create_contact') expect(JSON.parse(String(h.calls[0].init.body))).toEqual({ properties: input.properties, associations: [] });
    if (action === 'search_contacts') expect(JSON.parse(String(h.calls[0].init.body)).after).toBe(50);
  }
});

test('HubSpot user-level schema discovery fails explicitly without unsupported calls or fallback privileges', async () => {
  const h = harness();
  expect(await h.run('get_schema')).toMatchObject({ ok: false, error: { code: 'UNSUPPORTED_OPERATION' } });
  expect(h.calls).toHaveLength(0);
  expect(await h.run('get_schema', {}, { accessToken: 'legacy' })).toMatchObject({ ok: true });
  expect(h.calls[0].url.pathname).toBe('/crm/v3/properties/contacts');
});

test('HubSpot user-level verification requires current token access and does not return contact data', async () => {
  const h = harness({ results: [{ id: '123', properties: { email: 'private@example.test' } }] });
  expect(await h.run('verify_connection')).toEqual({ ok: true, data: identity });
  expect(h.calls[0].url.searchParams.get('limit')).toBe('1');
  const revoked = harness({ message: 'expired' }, 401);
  expect(await revoked.run('verify_connection')).toMatchObject({ ok: false, error: { code: 'unauthorized' } });
});

test('Missing or malformed user identity cannot fall back to account-wide routes', async () => {
  for (const value of [undefined, {}, { id: '', label: 'User' }, { id: '42', label: 42 }]) {
    const h = harness();
    expect(await h.run('list_contacts', {}, { ...userCredentials, hubspotUserIdentity: value })).toMatchObject({ ok: false, error: { code: 'unauthorized' } });
    expect(h.calls, JSON.stringify(value)).toHaveLength(0);
  }
});

test('HubSpot validation errors name the rejected properties without echoing submitted values', async () => {
  for (const reply of [
    { status: 'error', category: 'VALIDATION_ERROR', message: 'Property values were not valid: [{"isValid":false,"message":"private-value is required","error":"REQUIRED","name":"hs_legal_basis"}]' },
    { status: 'error', category: 'VALIDATION_ERROR', message: 'private-value', errors: [{ message: 'private-detail', context: { propertyName: ['hs_legal_basis'] } }] },
  ]) {
    const result = await harness(reply, 400).run('create_contact', { properties: { firstname: 'private-value' } });
    expect(result).toMatchObject({ ok: false, error: { code: 'PROVIDER_REJECTED', message: 'HubSpot returned HTTP 400 [VALIDATION_ERROR hs_legal_basis]' } });
    expect(JSON.stringify(result)).not.toContain('private-');
  }
});

test('HubSpot error parsing drops unknown categories and malformed property names', async () => {
  for (const [reply, suffix] of [
    [{ category: 'PRIVATE_CATEGORY', errors: [{ context: { propertyName: ['email'] } }] }, ''],
    [{ category: 'VALIDATION_ERROR', errors: [{ context: { propertyName: ['private value\n'] } }] }, ' [VALIDATION_ERROR]'],
    [{ category: 'VALIDATION_ERROR', message: 'Property values were not valid: [{broken' }, ' [VALIDATION_ERROR]'],
  ] as const) {
    const result = await harness(reply, 400).run('create_contact', { properties: { firstname: 'Fixture' } });
    expect(result).toMatchObject({ ok: false, error: { message: `HubSpot returned HTTP 400${suffix}` } });
    expect(JSON.stringify(result)).not.toContain('private');
  }
});
