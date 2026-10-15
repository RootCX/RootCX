import { expect, test } from 'bun:test';
import { createRuntime } from '../src/runtime';
import gmail from '../src/providers/gmail';
import stripe from '../src/providers/stripe';
import type { Plugin } from '../src/types';
async function run(body: unknown, plugin: Plugin = gmail, raw = false) {
  let calls = 0;
  const rpc = createRuntime(plugin, (async () => {
    calls++;
    return new Response(raw ? String(body) : JSON.stringify(body), { status: 403, headers: { 'x-private-diagnostic': 'private-header' } });
  }) as unknown as typeof fetch);
  const result = await rpc.__integration({ action: plugin.id === 'gmail' ? 'create_draft' : 'get_customer', input: plugin.id === 'gmail' ? { to: ['fixture@example.com'], subject: 'Synthetic', text: '' } : { customerId: 'cus_fixture' }, connectionId: crypto.randomUUID(), userCredentials: { accessToken: 'private-credential' } });
  expect(calls).toBe(1);
  return result;
}
test('Google 403 retains the stable code and exposes only an allowlisted reason', async () => {
  const result = await run({ error: { message: 'private-provider-message', errors: [{ reason: 'insufficientPermissions', message: 'private-detail', location: 'private-location' }], status: 'PERMISSION_DENIED' } });
  expect(result).toMatchObject({ ok: false, error: { code: 'FORBIDDEN', message: 'Gmail returned HTTP 403 [insufficientPermissions]', status: 403, outcome: 'rejected' } });
  expect(JSON.stringify(result)).not.toContain('private-');
});
test.each([
  [{ error: { errors: [{ reason: 'private-reason' }], status: 'PERMISSION_DENIED' } }, ' [PERMISSION_DENIED]'],
  [{ error: { errors: [{ reason: 'private-reason' }], status: 'PRIVATE_STATUS' } }, ''],
  [{ error: { errors: [{ reason: { value: 'forbidden' } }] } }, ''],
  [{ error: { errors: [{ reason: 'forbidden\nprivate-injection' }] } }, ''],
  [{ reason: 'insufficientPermissions', details: 'private-top-level' }, ''],
])('Google error parsing rejects unknown and malformed detail %#', async (body, suffix) => {
  const result = await run(body);
  expect(result).toMatchObject({ error: { code: 'FORBIDDEN', message: `Gmail returned HTTP 403${suffix}` } });
  expect(JSON.stringify(result)).not.toContain('private-');
});
test('Malformed or oversized Google errors preserve HTTP403 without exposing body data', async () => {
  for (const body of ['private-unparseable', JSON.stringify({ error: { message: 'x'.repeat(65536), status: 'PERMISSION_DENIED' } })]) {
    expect(await run(body, gmail, true)).toMatchObject({ error: { code: 'FORBIDDEN', message: 'Gmail returned HTTP 403' } });
  }
});
test('Google-looking reasons from another provider are never surfaced', async () => {
  expect(await run({ error: { status: 'PERMISSION_DENIED' } }, stripe)).toMatchObject({ error: { code: 'FORBIDDEN', message: 'Stripe returned HTTP 403' } });
});
