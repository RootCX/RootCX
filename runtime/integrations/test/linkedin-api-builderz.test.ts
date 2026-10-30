import { expect, test } from 'bun:test';
import plugin from '../src/providers/linkedin-api-builderz';
import { createRuntime, manifest } from '../src/runtime';

function harness(reply: unknown = { posts: [] }) {
  const calls: { url: URL; init: RequestInit }[] = [];
  const rpc = createRuntime(plugin, (async (url: URL, init: RequestInit) => {
    calls.push({ url: new URL(url), init });
    return reply instanceof Response ? reply : Response.json(reply);
  }) as unknown as typeof fetch);
  return { calls, rpc, run: (action: string, input = {}) => rpc.__integration({ action, input, connectionId: crypto.randomUUID(), userCredentials: { accessToken: 'fixture-private-key' } }) };
}

test.each([
  ['list_profile_posts', { profileUrl: 'https://linkedin.com/in/example' }, 'profile_updates', 'posts', 'profile_url', 'https://www.linkedin.com/in/example/'],
  ['list_company_posts', { companyUrl: 'https://www.linkedin.com/company/example/' }, 'company_updates', 'posts', 'company_url', 'https://www.linkedin.com/company/example/'],
  ['list_post_reactions', { reactionsUrn: 'urn:li:ugcPost:456' }, 'post_reactions', 'reactions', 'reactionsUrn', 'urn:li:ugcPost:456'],
  ['list_post_comments', { commentsUrn: 'urn:li:fsd_socialDetail:(urn:li:ugcPost:456,urn:li:ugcPost:456,urn:li:highlightedReply:-)' }, 'post_comments', 'comments', 'commentsUrn', 'urn:li:fsd_socialDetail:(urn:li:ugcPost:456,urn:li:ugcPost:456,urn:li:highlightedReply:-)'],
] as const)('%s sends one authenticated request and exposes explicit continuation', async (action, input, endpoint, field, param, value) => {
  const h = harness({ success: true, [field]: [{ title: 'Fixture' }], pagination: { start: 20, count: 10, total: 31 }, ...(action === 'list_profile_posts' ? { paginationToken: 'next==' } : {}) });
  expect(await h.run(action, { ...input, page: 3, ...(action === 'list_profile_posts' ? { paginationToken: 'before==' } : {}) })).toMatchObject({ ok: true, data: { items: [{ title: 'Fixture' }], page: 3, nextPage: 4, hasMore: true, truncated: false } });
  expect(h.calls).toHaveLength(1);
  const { url, init } = h.calls[0];
  expect(url.origin).toBe('https://realtime-linkedin-bulk-data.p.rapidapi.com');
  expect(url.pathname).toBe(`/${endpoint}`);
  expect(Object.fromEntries(url.searchParams)).toEqual({ [param]: value, page: '3', ...(action === 'list_profile_posts' ? { paginationToken: 'before==' } : {}) });
  const headers = new Headers(init.headers);
  expect(headers.get('x-rapidapi-key')).toBe('fixture-private-key');
  expect(headers.get('x-rapidapi-host')).toBe(url.hostname);
  expect(headers.has('authorization')).toBe(false);
  expect(init.redirect).toBe('error');
});

test('person lookup is read-only POST with validated profile', async () => {
  const h = harness({ success: true, data: { firstName: 'Fixture', headline: 'Engineer' } });
  expect(await h.run('get_person', { profileUrl: 'https://linkedin.com/in/example' })).toMatchObject({ ok: true, data: { person: { firstName: 'Fixture' } } });
  expect(h.calls[0].init.method).toBe('POST');
  expect(JSON.parse(String(h.calls[0].init.body))).toEqual({ link: 'https://www.linkedin.com/in/example/' });
  expect(Object.values(plugin.actions).every(action => action.effect === 'read')).toBe(true);
});

test('missing pagination is unknown, not a false completed collection', async () => {
  const input = { companyUrl: 'https://linkedin.com/company/example/' };
  expect(await harness().run('list_company_posts', input)).toMatchObject({ ok: true, data: { hasMore: null, nextPage: 2 } });
  expect(await harness().run('list_company_posts', { ...input, page: 10000 })).toMatchObject({ ok: true, data: { hasMore: null, nextPage: null, truncated: true } });
  expect(await harness({ posts: [], pagination: { start: 0, count: 0, total: 0 } }).run('list_company_posts', input)).toMatchObject({ ok: true, data: { hasMore: false, nextPage: null } });
});

test.each([{ success: false, posts: [] }, { error: 'private vendor text', posts: [] }, {}, { posts: null }, { posts: [null] }, { posts: [], pagination: { start: 0, count: 0, total: 5 } }])('provider rejection or malformed pages never become empty success: %j', async response => {
  const result = await harness(response).run('list_profile_posts', { profileUrl: 'https://linkedin.com/in/example/' });
  expect(result.ok).toBe(false);
  expect(JSON.stringify(result)).not.toContain('private vendor text');
});

test.each(['https://evil.test/in/example', 'https://linkedin.com.evil.test/in/example', 'https://linkedin.com/company/example', 'https://linkedin.com/in/../company', 'https://linkedin.com/in/a%2Fb', 'https://linkedin.com/in/a\\b', 'http://linkedin.com/in/example', 'https://linkedin.com/in/example?url=https://evil.test'])('rejects invalid sources without calling provider: %s', async profileUrl => {
  const h = harness();
  expect(await h.run('list_profile_posts', { profileUrl })).toMatchObject({ ok: false, error: { code: 'INVALID_INPUT' } });
  expect(h.calls).toHaveLength(0);
});

test.each([[401, 'unauthorized'], [403, 'FORBIDDEN'], [429, 'RATE_LIMITED'], [502, 'PROVIDER_UNAVAILABLE'], [503, 'PROVIDER_UNAVAILABLE']] as const)('HTTP %i stays an error, without retries', async (status, code) => {
  const h = harness(new Response('fixture-private-key', { status, headers: { 'Retry-After': '7' } }));
  const result = await h.run('verify_connection');
  expect(result).toMatchObject({ ok: false, error: { code } });
  expect(JSON.stringify(result)).not.toContain('fixture-private-key');
  expect(h.calls).toHaveLength(1);
});

test('credentials require a successful probe; reconnection rotates the one key slot', async () => {
  const h = harness();
  const auth = await h.rpc.__auth_credentials({ userId: crypto.randomUUID(), credentials: { accessToken: 'fixture-private-key' } });
  expect(auth.label).toBe('API Builderz [rapidapi]');
  expect(h.calls).toHaveLength(1);
  expect(h.calls[0].url.searchParams.get('company_url')).toBe('https://www.linkedin.com/company/linkedin/');
  const invalid = harness({ success: false });
  await expect(invalid.rpc.__auth_credentials({ userId: crypto.randomUUID(), credentials: { accessToken: 'fixture-private-key' } })).rejects.toThrow();
  const malformed = harness();
  await expect(malformed.rpc.__auth_credentials({ userId: crypto.randomUUID(), credentials: { accessToken: 'bad\r\nkey' } })).rejects.toThrow();
  expect(malformed.calls).toHaveLength(0);
  expect(manifest(plugin).userAuth.plugin.qualification).toBe('requires-live-verification');
});

test('profile pagination uses the opaque token even when the provider reports total zero', async () => {
  const h = harness({ success: true, status: 200, posts: [{ urn: 'urn:li:activity:123', reactionsUrn: 'urn:li:ugcPost:456', commentsUrn: 'urn:li:fsd_socialDetail:(urn:li:ugcPost:456,urn:li:ugcPost:456,urn:li:highlightedReply:-)' }], pagination: { start: 20, count: 20, total: 0 }, paginationToken: 'next+/==' });
  expect(await h.run('list_profile_posts', { profileUrl: 'https://linkedin.com/in/example/', page: 2, paginationToken: 'previous+/==' })).toMatchObject({ ok: true, data: { hasMore: true, nextPage: 3, nextPaginationToken: 'next+/==', items: [{ urn: 'urn:li:activity:123', reactionsUrn: 'urn:li:ugcPost:456' }] } });
  expect(h.calls[0].url.searchParams.get('paginationToken')).toBe('previous+/==');
  expect(h.calls[0].url.searchParams.get('page')).toBe('2');
});

test('zero totals cannot hide returned records, and malformed continuations fail closed', async () => {
  const input = { profileUrl: 'https://linkedin.com/in/example/' };
  expect(await harness({ posts: [{ urn: 'urn:li:activity:1' }], pagination: { start: 0, count: 20, total: 0 } }).run('list_profile_posts', input)).toMatchObject({ ok: true, data: { hasMore: null, nextPage: null, nextPaginationToken: null, truncated: true } });
  for (const paginationToken of [42, {}, 'x'.repeat(8193)]) {
    expect(await harness({ posts: [], paginationToken }).run('list_profile_posts', input)).toMatchObject({ ok: false, error: { code: 'INVALID_RESPONSE' } });
  }
});

test('interaction actions reject ambiguous post IDs and malformed provider identifiers before HTTP', async () => {
  for (const [action, input] of [
    ['list_post_reactions', { postUrn: 'urn:li:activity:123' }],
    ['list_post_comments', { postUrn: 'urn:li:activity:123' }],
    ['list_post_reactions', { reactionsUrn: 'https://example.test' }],
    ['list_post_comments', { commentsUrn: 'urn:li:fsd_socialDetail:(urn:li:ugcPost:1,urn:li:ugcPost:2,urn:li:highlightedReply:-)' }],
  ] as const) {
    const h = harness();
    expect(await h.run(action, input)).toMatchObject({ ok: false, error: { code: 'INVALID_INPUT' } });
    expect(h.calls).toHaveLength(0);
  }
});


test('profile page numbers alone cannot silently repeat the first page', async () => {
  const h = harness();
  expect(await h.run('list_profile_posts', { profileUrl: 'https://linkedin.com/in/example/', page: 2 })).toMatchObject({ ok: false, error: { code: 'INVALID_INPUT' } });
  expect(h.calls).toHaveLength(0);
});

test('provider HTTP-200 error statuses and ambiguous success values are rejected', async () => {
  for (const reply of [{ status: 500, posts: [] }, { success: 'false', posts: [] }, { success: null, posts: [] }]) {
    expect(await harness(reply).run('verify_connection')).toMatchObject({ ok: false, error: { code: 'PROVIDER_REJECTED' } });
  }
});
