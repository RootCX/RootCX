import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { createProviderProxy } from '../provider-proxy.mjs';
import { providerError } from '../opencode.mjs';

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
