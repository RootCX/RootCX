import { expect, test } from 'bun:test';
import { createRuntime } from '../src/runtime';
import stripe from '../src/providers/stripe';
import type { ObjectValue } from '../src/types';
function harness(response: unknown = { id: 'in_fixture' }) {
  const calls: { url: URL; init: RequestInit }[] = [];
  const rpc = createRuntime(stripe, (async (url: URL | string, init: RequestInit) => {
    calls.push({ url: new URL(url), init });
    if (response instanceof Error) throw response;
    return Response.json(response);
  }) as typeof fetch);
  return { calls, run: (action: string, input: ObjectValue) => rpc.__integration({ action, input, connectionId: crypto.randomUUID(), userCredentials: { accessToken: 'fixture-only' } }) };
}
test('Stripe reads encode nested filters and expansions as query parameters', async () => {
  const h = harness();
  expect(await h.run('api_read', { path: '/v1/invoices', parameters: { created: { gte: 123 }, expand: ['data.customer'], limit: 2 } })).toMatchObject({ ok: true });
  expect(h.calls[0].url.origin).toBe('https://api.stripe.com');
  expect(Object.fromEntries(h.calls[0].url.searchParams)).toEqual({ 'created[gte]': '123', 'expand[0]': 'data.customer', limit: '2' });
  expect(h.calls[0].init.body).toBeUndefined();
  expect(new Headers(h.calls[0].init.headers).get('authorization')).toBe('Bearer fixture-only');
});
test.each(['https://evil.test/v1/customers', '//evil.test/v1/customers', '/v1/../oauth/token', '/v1/%2e%2e/account', '/v1/customers?x=1', '/v1/customers#x', '/v1//customers', '/v1/customers\\evil', '/oauth/token'])('Stripe refuses unsafe paths: %s', async path => {
  const h = harness();
  expect(await h.run('api_write', { path, method: 'POST', idempotencyKey: 'fixture' })).toMatchObject({ ok: false, error: { code: 'INVALID_INPUT' } });
  expect(h.calls).toHaveLength(0);
});
test.each([
  { path: '/v1/refunds', method: 'POST' },
  { path: '/v1/refunds', method: 'GET', idempotencyKey: 'fixture' },
  { path: '/v1/refunds', method: 'POST', idempotencyKey: 'bad\r\nHeader: x' },
  { path: '/v1/refunds', method: 'POST', idempotencyKey: 'fixture', headers: { 'Stripe-Account': 'acct_other' } },
  { path: '/v1/refunds', method: 'POST', idempotencyKey: 'fixture', parameters: { 'metadata[x]': 'injection' } },
  { path: '/v1/refunds', method: 'POST', idempotencyKey: 'fixture', parameters: { metadata: { constructor: 'bad' } } },
])('Stripe rejects unsafe write inputs without network access %#', async input => {
  const h = harness();
  expect(await h.run('api_write', input)).toMatchObject({ ok: false, error: { code: 'INVALID_INPUT' } });
  expect(h.calls).toHaveLength(0);
});
test.each(['/v1/refunds', '/v2/core/accounts'])('Stripe uses the API family encoding and stable idempotency: %s', async path => {
  const h = harness();
  const parameters = { metadata: { fixture: 'yes' } };
  expect(await h.run('api_write', { path, method: 'POST', parameters, idempotencyKey: 'operation-1' })).toMatchObject({ ok: true });
  const headers = new Headers(h.calls[0].init.headers);
  expect(headers.get('Idempotency-Key')).toBe('operation-1');
  expect(headers.get('Stripe-Version')).toBe('2026-08-26.dahlia');
  expect(path.startsWith('/v2/') ? JSON.parse(String(h.calls[0].init.body)) : Object.fromEntries(new URLSearchParams(String(h.calls[0].init.body)))).toEqual(path.startsWith('/v2/') ? parameters : { 'metadata[fixture]': 'yes' });
});
test('Stripe draft invoice disables automatic advancement and binds the selected customer', async () => {
  const h = harness();
  expect(await h.run('create_invoice', { customerId: 'cus_fixture', idempotencyKey: 'draft-1', parameters: { auto_advance: true, customer: 'cus_other', description: 'Synthetic' } })).toMatchObject({ ok: true });
  expect(Object.fromEntries(new URLSearchParams(String(h.calls[0].init.body)))).toEqual({ customer: 'cus_fixture', auto_advance: 'false', pending_invoice_items_behavior: 'exclude', description: 'Synthetic' });
});
test.each([['finalize_invoice', 'POST', '/finalize'], ['send_invoice', 'POST', '/send'], ['pay_invoice', 'POST', '/pay'], ['void_invoice', 'POST', '/void'], ['mark_invoice_uncollectible', 'POST', '/mark_uncollectible'], ['delete_draft_invoice', 'DELETE', '']])('Stripe invoice action %s fixes its endpoint and write effect', async (name, method, suffix) => {
  const h = harness();
  expect(stripe.actions[name].effect).toBe('write');
  expect(await h.run(name, { invoiceId: 'in_fixture', idempotencyKey: 'invoice-operation' })).toMatchObject({ ok: true });
  expect(h.calls[0].url.pathname).toBe(`/v1/invoices/in_fixture${suffix}`);
  expect(h.calls[0].init.method).toBe(method);
});
test('Stripe redacts nested credential and client secrets from expanded responses', async () => {
  const h = harness({ id: 'pi_fixture', client_secret: 'sensitive', expanded: [{ secret: 'sensitive', metadata: { copied: 'sk_test_abcdefghijklmnop' } }], amount: 100 });
  expect(await h.run('api_read', { path: '/v1/payment_intents/pi_fixture' })).toMatchObject({ ok: true, data: { id: 'pi_fixture', amount: 100, client_secret: '[REDACTED]', expanded: [{ secret: '[REDACTED]', metadata: { copied: '[REDACTED]' } }] } });
});
test('Stripe uncertain payment writes are never retried automatically', async () => {
  const h = harness(new TypeError('connection lost'));
  expect(await h.run('pay_invoice', { invoiceId: 'in_fixture', idempotencyKey: 'pay-fixture' })).toMatchObject({ ok: false, error: { code: 'TRANSPORT_ERROR', outcome: 'unknown' } });
  expect(h.calls).toHaveLength(1);
});

test('Stripe v1 preserves explicit empty collection clearing and metadata spaces', async () => {
  const h = harness();
  expect(await h.run('api_write', { path: '/v1/customers/cus_fixture', method: 'POST', idempotencyKey: 'clear-1', parameters: { metadata: { 'customer reference': '' }, preferred_locales: [] } })).toMatchObject({ ok: true });
  expect(Object.fromEntries(new URLSearchParams(String(h.calls[0].init.body)))).toEqual({ 'metadata[customer reference]': '', preferred_locales: '' });
});
test('Stripe v2 DELETE preserves JSON body and idempotency', async () => {
  const h = harness();
  expect(await h.run('api_write', { path: '/v2/core/event_destinations/ed_fixture', method: 'DELETE', parameters: {}, idempotencyKey: 'delete-1' })).toMatchObject({ ok: true });
  expect(JSON.parse(String(h.calls[0].init.body))).toEqual({});
  expect(new Headers(h.calls[0].init.headers).get('Idempotency-Key')).toBe('delete-1');
});
