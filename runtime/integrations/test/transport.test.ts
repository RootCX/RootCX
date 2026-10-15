import { expect, test } from 'bun:test';
import { context } from '../src/http';
import { createRuntime } from '../src/runtime';
import { plugins } from '../src/registry';

const stripe = plugins.find(plugin => plugin.id === 'stripe')!;

test.each([204, 205])('Successful HTTP %s writes do not consume a nonexistent response body', async status => {
  let read = false;
  let cancelled = false;
  const response = new Response(null, { status });
  // Bun can expose a stream even for a no-content response.
  Object.defineProperty(response, 'body', { value: new ReadableStream({
    pull(controller) { read = true; controller.error(new Error('No response content')); },
    cancel() { cancelled = true; },
  }, { highWaterMark: 0 }) });
  const ctx = context(stripe, { accessToken: 'fixture' }, crypto.randomUUID(), (async () => response) as unknown as typeof fetch, 'write');
  await expect(ctx.request({ path: '/fixture', method: 'PATCH', json: {} })).resolves.toEqual({});
  expect(read).toBe(false);
  expect(cancelled).toBe(true);
});

test.each(['read', 'write'] as const)('POST %s failures preserve action semantics', async effect => {
  let calls = 0;
  const ctx = context(stripe, { accessToken: 'fixture' }, crypto.randomUUID(), (async () => {
    calls++;
    throw new Error('private upstream details');
  }) as unknown as typeof fetch, effect);
  await expect(ctx.request({ path: '/search', method: 'POST', json: {} })).rejects.toMatchObject({
    code: 'TRANSPORT_ERROR', outcome: effect === 'write' ? 'unknown' : 'rejected',
  });
  expect(calls).toBe(1);
});

test.each([['0', 1], ['999999999', 86400], ['Infinity', 60], ['invalid', 60]])('Retry-After %s uses the same bounded cooldown in responses and subsequent requests', async (header, expected) => {
  let calls = 0;
  const ctx = context(stripe, { accessToken: 'fixture' }, crypto.randomUUID(), (async () => {
    calls++;
    return new Response('', { status: 429, headers: { 'Retry-After': String(header) } });
  }) as unknown as typeof fetch);
  for (let attempt = 0; attempt < 2; attempt++) {
    await expect(ctx.request({ path: '/' })).rejects.toMatchObject({ code: 'RATE_LIMITED', retryAfter: expected });
  }
  expect(calls).toBe(1);
});

test('Oversized responses release the upstream stream', async () => {
  let cancelled = false;
  const ctx = context(stripe, { accessToken: 'fixture' }, crypto.randomUUID(), (async () => new Response(
    new ReadableStream({ cancel() { cancelled = true; } }), { headers: { 'Content-Length': '10485761' } },
  )) as unknown as typeof fetch);
  await expect(ctx.request({ path: '/' })).rejects.toMatchObject({ code: 'RESPONSE_TOO_LARGE' });
  expect(cancelled).toBe(true);
});

test.each([
  null, { access_token: '' }, { access_token: '  ' }, { access_token: 'x\r\ny' },
  { access_token: 'valid', refresh_token: {} }, { access_token: 'valid', refresh_token: '' },
  { access_token: 'valid', expires_in: 'Infinity' }, { access_token: 'valid', expires_in: -1 },
  { access_token: 'valid', expires_in: 0 },
])('Malformed OAuth grants fail before account verification: %j', async token => {
  let calls = 0;
  const plugin = plugins.find(p => p.id === 'slack')!;
  const rpc = createRuntime(plugin, (async () => { calls++; return Response.json(token); }) as unknown as typeof fetch);
  await expect(rpc.__auth_callback({ config: { clientId: 'fixture', clientSecret: 'fixture' }, query: { code: 'fixture' } })).rejects.toThrow();
  expect(calls).toBe(1);
});

test('Unexpected adapter failure after a provider accepted a write remains uncertain', async () => {
  let calls = 0;
  const plugin = { ...stripe, actions: { create: {
    ...stripe.actions.create_customer,
    async run(ctx: import('../src/types').Context) {
      await ctx.request({ path: '/v1/customers', method: 'POST', json: {} });
      throw new Error('private parsing detail');
    },
  } } };
  const rpc = createRuntime(plugin, (async () => { calls++; return Response.json({ id: 'created' }); }) as unknown as typeof fetch);
  const result = await rpc.__integration({ action: 'create', input: { name: 'Fixture', idempotencyKey: 'intent' }, connectionId: crypto.randomUUID(), userCredentials: { accessToken: 'fixture' } });
  expect(result).toMatchObject({ ok: false, error: { code: 'CONNECTOR_ERROR', outcome: 'unknown' } });
  expect(JSON.stringify(result)).not.toContain('private parsing detail');
  expect(calls).toBe(1);
});
