import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { createProviderProxy } from '../provider-proxy.mjs';
import { providerError } from '../opencode.mjs';

test('native SDK beta requests reach only the configured gateway with its private key', async t => {
  const requests = [];
  const upstream = createServer(async (req, res) => {
    let body = ''; for await (const chunk of req) body += chunk;
    requests.push({ path: req.url, key: req.headers['x-api-key'], beta: req.headers['anthropic-beta'], body });
    res.writeHead(200, { 'content-type': 'application/json' }); res.end('{}');
  });
  await new Promise(resolve => upstream.listen(0, '127.0.0.1', resolve));
  const proxy = createProviderProxy({ endpoint: `http://127.0.0.1:${upstream.address().port}/fixed/messages`, apiKey: 'private-test-key' }, 'scoped-test-key');
  await new Promise(resolve => proxy.listen(0, '127.0.0.1', resolve));
  t.after(() => { upstream.closeAllConnections(); upstream.close(); proxy.closeAllConnections(); proxy.close(); });
  const send = (path, key = 'scoped-test-key') => fetch(`http://127.0.0.1:${proxy.address().port}${path}`, {
    method: 'POST', headers: { 'x-api-key': key, 'anthropic-beta': 'interleaved-thinking-2025-05-14' }, body: '{"model":"fixture"}',
  });
  for (const path of ['/v1/messages?beta=true', '/v1/messages/count_tokens?beta=true']) {
    const response = await send(path); assert.equal(response.status, 200); await response.text();
  }
  assert.deepEqual(requests.map(request => request.path), ['/fixed/messages', '/fixed/messages/count_tokens']);
  assert.ok(requests.every(request => request.key === 'private-test-key' && request.beta === 'interleaved-thinking-2025-05-14' && request.body === '{"model":"fixture"}'));
  for (const path of ['/v1/messages?redirect=http://other', '/v1/messages?beta=true&url=http://other', '/v1/messages?beta=true?extra', '/v1/messages/other']) {
    assert.equal((await send(path)).status, 403);
  }
  assert.equal((await send('/v1/messages', 'wrong-key')).status, 403);
  assert.equal(requests.length, 2);
});

test('offline gateway fails without model retries; upstream overload remains retryable', async t => {
  const upstream = createServer((req, res) => { res.writeHead(503); res.end('{"error":{"message":"Busy"}}'); });
  await new Promise(resolve => upstream.listen(0, '127.0.0.1', resolve));
  const endpoint = `http://127.0.0.1:${upstream.address().port}/v1/messages`;
  const proxy = createProviderProxy({ endpoint, apiKey: 'private-test-key' }, 'scoped-test-key');
  await new Promise(resolve => proxy.listen(0, '127.0.0.1', resolve));
  t.after(() => { upstream.closeAllConnections(); upstream.close(); proxy.closeAllConnections(); proxy.close(); });
  const send = () => fetch(`http://127.0.0.1:${proxy.address().port}/v1/messages`, { method: 'POST', headers: { 'x-api-key': 'scoped-test-key' }, body: '{}' });
  const busy = await send();
  assert.equal(busy.status, 503);
  await busy.text();
  upstream.closeAllConnections();
  await new Promise(resolve => upstream.close(resolve));
  const offline = await send();
  const body = await offline.text();
  assert.equal(offline.status, 400);
  assert.equal(providerError({ data: { statusCode: offline.status, responseBody: body } }), 'AI_CONFIGURATION');
  assert.ok(!body.includes('private-test-key'));
  assert.ok(!body.includes(endpoint));
});
