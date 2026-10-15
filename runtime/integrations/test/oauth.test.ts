import { expect, test } from 'bun:test';
import { plugins } from '../src/registry';
import { createRuntime } from '../src/runtime';
import { context } from '../src/http';

const identities: Record<string, unknown> = {
  notion: { id: 'bot1', bot: { workspace_name: 'Fixture' } },
  hubspot: { portalId: 123 },
  salesforce: { organization_id: 'org1', user_id: 'user1', preferred_username: 'fixture@example.com' },
  airtable: { id: 'usr1' }, slack: { ok: true, team_id: 'team1', user_id: 'user1', team: 'Fixture', user: 'bot' },
};
for (const plugin of plugins.filter(p => p.oauth)) {
  test(`${plugin.id} OAuth transmits its real provider protocol and verifies the account before returning credentials`, async () => {
    const requests: { url: URL; init: RequestInit }[] = [];
    const fetcher = (async (url: string | URL, init: RequestInit) => {
      requests.push({ url: new URL(String(url)), init });
      if (new URL(String(url)).pathname === '/drive/v3/about') return Response.json({ user: { permissionId: 'fixture-id' } });
      return Response.json(requests.length === 1 ? { access_token: 'access-fixture', refresh_token: 'refresh-fixture', expires_in: 3600, instance_url: 'https://fixture.my.salesforce.com' } : identities[plugin.id] ?? { sub: 'user1', email: 'fixture@example.com' });
    }) as unknown as typeof fetch;
    const rpc = createRuntime(plugin, fetcher);
    const config = { clientId: 'client-fixture', clientSecret: 'secret-fixture' };
    const params = { config, callbackUrl: 'https://app.example.com/callback', state: 'state1', codeChallenge: 'challenge1', userId: crypto.randomUUID() };
    const start = await rpc.__auth_start(params);
    const url = new URL(start.url!);
    expect(url.origin + url.pathname).toBe(plugin.oauth!.authorizeUrl);
    expect(url.searchParams.get('state')).toBe('state1');
    if (plugin.id === 'notion') expect(url.searchParams.get('owner')).toBe('user');
    if (plugin.id === 'gmail') expect(url.searchParams.get('scope')?.split(' ')).toEqual(['openid', 'email', 'https://www.googleapis.com/auth/gmail.readonly', 'https://www.googleapis.com/auth/gmail.compose']);
    // Legacy freeBusy.query needs its own permission; events alone does not authorize it.
    if (plugin.id === 'google_calendar') expect(url.searchParams.get('scope')?.split(' ')).toEqual([
      'openid', 'email',
      'https://www.googleapis.com/auth/calendar.events',
      'https://www.googleapis.com/auth/calendar.calendarlist.readonly',
      'https://www.googleapis.com/auth/calendar.events.freebusy',
    ]);
    expect(url.searchParams.get('scope')).toBe(plugin.oauth!.scopes.join(plugin.oauth!.scopeSeparator ?? ' '));
    if (plugin.oauth!.pkce) {
      expect(url.searchParams.get('code_challenge_method')).toBe('S256');
      await expect(rpc.__auth_start({ ...params, codeChallenge: undefined })).rejects.toThrow('PKCE');
    }
    const result = await rpc.__auth_callback({ ...params, query: { code: 'code1' }, codeVerifier: 'verifier1' });
    expect(requests).toHaveLength(plugin.id === 'google_drive' ? 3 : 2);
    expect(requests[0].url.toString()).toBe(plugin.oauth!.tokenUrl);
    expect(new Headers(requests[0].init.headers).get('Notion-Version')).toBe(plugin.id === 'notion' ? '2025-09-03' : null);
    if (plugin.id === 'notion') expect(new Headers(requests[1].init.headers).get('Notion-Version')).toBe('2025-09-03');
    const fields = plugin.oauth!.tokenEncoding === 'json' ? JSON.parse(String(requests[0].init.body)) : Object.fromEntries(new URLSearchParams(String(requests[0].init.body)));
    expect(fields.code).toBe('code1');
    if (plugin.oauth!.pkce) expect(fields.code_verifier).toBe('verifier1');
    if (plugin.oauth!.clientAuth === 'basic') {
      expect(new Headers(requests[0].init.headers).get('authorization')).toBe(`Basic ${Buffer.from('client-fixture:secret-fixture').toString('base64')}`);
      expect(fields.client_secret).toBeUndefined();
    } else expect(fields.client_secret).toBe('secret-fixture');
    expect(result.credentials).toMatchObject({ accessToken: 'access-fixture', refreshToken: 'refresh-fixture' });
    expect(result.label).toContain('[');
  });
}

test('OAuth denies cancellation, empty grants and provider errors without echoing secrets', async () => {
  const plugin = plugins.find(p => p.id === 'slack')!;
  for (const payload of [{ ok: false, error: 'bad_secret' }, { access_token: null }]) {
    const rpc = createRuntime(plugin, (async () => Response.json(payload)) as unknown as typeof fetch);
    await expect(rpc.__auth_callback({ config: { clientId: 'id', clientSecret: 'secret' }, query: { code: 'code' } })).rejects.toThrow('did not issue');
    await expect(rpc.__auth_callback({ query: { error: 'access_denied' } })).rejects.toThrow('not granted');
  }
});

test('Local connection budgets reject before network I/O without leaking a concurrency slot', async () => {
  let calls = 0;
  const plugin = { ...plugins.find(p => p.id === 'stripe')!, requestsPerSecond: 1 };
  const ctx = context(plugin, { accessToken: 'fixture' }, crypto.randomUUID(), (async () => { calls++; return Response.json({}); }) as unknown as typeof fetch);
  await expect(ctx.request({ path: '/', headers: { host: 'evil' } })).rejects.toThrow('Reserved');
  await ctx.request({ path: '/' });
  await expect(ctx.request({ path: '/' })).rejects.toMatchObject({ code: 'RATE_LIMITED' });
  expect(calls).toBe(1);
});

test('A reviewed plugin can sign requests without a Bearer-only factory constraint', async () => {
  let sent: RequestInit | undefined;
  const plugin = { ...plugins.find(p => p.id === 'stripe')!, authorize: (credential: { accessToken: string }, request: { method: string; url: URL }) => ({ 'X-Api-Key': credential.accessToken, 'X-Signed-Target': `${request.method} ${request.url.pathname}` }) };
  const ctx = context(plugin, { accessToken: 'fixture-key' }, crypto.randomUUID(), (async (_url: unknown, init: RequestInit) => { sent = init; return Response.json({}); }) as unknown as typeof fetch);
  await ctx.request({ path: '/v1/customers', method: 'POST', json: { name: 'A' } });
  const headers = new Headers(sent!.headers);
  expect(headers.get('authorization')).toBeNull();
  expect(headers.get('x-api-key')).toBe('fixture-key');
  expect(headers.get('x-signed-target')).toBe('POST /v1/customers');
});


test('Notion accepts an explicit null refresh only without expiry and still verifies identity', async () => {
  const plugin = plugins.find(p => p.id === 'notion')!;
  for (const token of [
    { access_token: 'access', refresh_token: null },
    { access_token: 'access', refresh_token: null, expires_in: 3600 },
    { access_token: 'access', refresh_token: '' },
  ]) {
    let calls = 0;
    const rpc = createRuntime(plugin, (async () => Response.json(++calls === 1 ? token : identities.notion)) as unknown as typeof fetch);
    const pending = rpc.__auth_callback({ config: { clientId: 'client', clientSecret: 'secret' }, query: { code: 'code' }, callbackUrl: 'https://example.com/callback' });
    if (token.refresh_token === null && !('expires_in' in token)) {
      const result = await pending;
      expect(result.credentials).toEqual({ accessToken: 'access' });
      expect(calls).toBe(2);
    } else {
      await expect(pending).rejects.toThrow('invalid refresh token');
      expect(calls).toBe(1);
    }
  }
});
