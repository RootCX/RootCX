import assert from 'node:assert/strict';
import test from 'node:test';
import { once } from 'node:events';
import { connect, createServer as tcpServer } from 'node:net';
import { createServer as httpServer, request } from 'node:http';
import { promisify } from 'node:util';
import { execFile } from 'node:child_process';
import { NetworkConfigSchema } from '@anthropic-ai/sandbox-runtime';
import { createResolvedAddressGuard } from '@anthropic-ai/sandbox-runtime/dist/sandbox/resolved-address-guard.js';
import { createHttpProxyServer } from '@anthropic-ai/sandbox-runtime/dist/sandbox/http-proxy.js';
import { createSocksProxyServer } from '@anthropic-ai/sandbox-runtime/dist/sandbox/socks-proxy.js';
import { directRequestOptions } from '@anthropic-ai/sandbox-runtime/dist/sandbox/parent-proxy.js';
import { workerPolicy, allowPublicDestination } from '../policy.mjs';

const network = workerPolicy({ readPaths: ['/opt/runtime'], home: '/tmp/worker-home', storageSocket: '/tmp/storage.sock' }).network;
const listen = server => new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const close = server => new Promise(resolve => server.close(resolve));

test('public API and mail ports reject IP spelling tricks and localhost', () => {
  assert.equal(NetworkConfigSchema.safeParse(network).success, true);
  for (const host of ['api.example.com', 'imap.example.com', 'smtp.example.com']) {
    for (const port of [443, 993, 587]) assert.equal(allowPublicDestination({ host, port }), true, `${host}:${port}`);
  }
  for (const host of ['10.0.0.1', '127.1', '2130706433', '0x7f000001', '0177.0.0.1', '169.254.169.254', '2852039166', '[::1]', '[::ffff:127.0.0.1]', '::1', 'localhost', 'x.localhost', 'localhost.', '', 'bad\nname']) {
    assert.equal(allowPublicDestination({ host, port: 443 }), false, JSON.stringify(host));
  }
  for (const port of [0, 22, 25, 80, 5432, 9200, 65535]) assert.equal(allowPublicDestination({ host: 'api.example.com', port }), false, String(port));
});

test('SRT denies private DNS answers including metadata and embedded IPv4', () => {
  const guard = createResolvedAddressGuard(network);
  for (const address of ['10.12.1.2', '172.16.0.1', '192.168.0.2', '100.64.0.1', 'fc00::1', 'fec0::1', '169.254.169.254', 'fd00:ec2::254', 'fd00:ec2::23', '127.0.0.1', '0.0.0.0', '::1', '::ffff:10.12.1.2', '64:ff9b::a0c:102', '2002:a0c:102::1', '64:ff9b:1::a00:1']) {
    assert.equal(guard.permits('api.example.com', address, 443), false, address);
  }
  assert.equal(guard.permits('api.example.com', '104.16.31.34', 443), true);
});

test('HTTP CONNECT and SOCKS enforce the same resolved-address policy', { timeout: 10000 }, async () => {
  let resolves = 0;
  const guard = createResolvedAddressGuard({ ...network, resolve: (_host, _options, callback) => {
    resolves++;
    callback(null, [{ address: '10.12.1.2', family: 4 }]);
  } });
  const options = { filter: (port, host) => allowPublicDestination({ host, port }), lookupFor: port => guard.lookupFor(port) };
  const proxy = createHttpProxyServer(options);
  const socks = createSocksProxyServer(options);
  const listener = tcpServer(socket => socks.handleConnection(socket));
  let client;
  try {
    await listen(proxy);
    await listen(listener);
    const status = await new Promise((resolve, reject) => {
      const socket = connect(proxy.address().port, '127.0.0.1', () => socket.write('CONNECT private.example.com:443 HTTP/1.1\r\nHost: private.example.com:443\r\n\r\n'));
      socket.once('data', chunk => { socket.destroy(); resolve(Number(chunk.toString().split(' ')[1])); });
      socket.once('error', reject);
    });
    assert.equal(status, 403);
    client = connect(listener.address().port, '127.0.0.1');
    await once(client, 'connect');
    client.write(Buffer.from([5, 1, 0]));
    const [hello] = await once(client, 'data');
    assert.equal(hello[1], 0);
    const host = Buffer.from('private.example.com');
    client.write(Buffer.concat([Buffer.from([5, 1, 0, 3, host.length]), host, Buffer.from([1, 187])]));
    const [reply] = await once(client, 'data');
    assert.equal(reply[1], 2);
    assert.equal(resolves, 2);
  } finally {
    client?.destroy();
    await socks.close();
    await close(listener);
    proxy.closeAllConnections();
    await close(proxy);
  }
});

test('HTTP forwarding uses the vetted address without a second DNS lookup', { timeout: 10000 }, async () => {
  const upstream = httpServer((_req, res) => res.end('pinned'));
  await listen(upstream);
  let lookups = 0;
  const port = upstream.address().port;
  const guard = createResolvedAddressGuard({ ...network, allowedDomains: [`127.0.0.1:${port}`], resolve: (_host, _options, callback) => {
    lookups++;
    callback(null, [{ address: lookups === 1 ? '127.0.0.1' : '10.12.1.2', family: 4 }]);
  } });
  try {
    const pinned = await directRequestOptions('api.example.com', port, guard.lookupFor(port), false);
    const body = await new Promise((resolve, reject) => {
      const req = request(pinned, res => { let data = ''; res.on('data', chunk => { data += chunk; }); res.on('end', () => resolve(data)); });
      req.on('error', reject); req.end();
    });
    assert.equal(body, 'pinned');
    assert.equal(lookups, 1);
  } finally { await close(upstream); }
});

test('public HTTPS works through the guarded SRT proxy', { skip: process.env.ROOTCX_TEST_PUBLIC_NETWORK !== '1', timeout: 30000 }, async () => {
  const guard = createResolvedAddressGuard(network);
  const proxy = createHttpProxyServer({ filter: (port, host) => allowPublicDestination({ host, port }), lookupFor: port => guard.lookupFor(port) });
  await listen(proxy);
  try {
    const { stdout } = await promisify(execFile)('curl', ['--silent', '--show-error', '--fail', '--noproxy', '', '--proxy', `http://127.0.0.1:${proxy.address().port}`, '--max-time', '20', 'https://registry.npmjs.org/-/ping']);
    assert.equal(typeof JSON.parse(stdout), 'object');
  } finally { proxy.closeAllConnections(); await close(proxy); }
});
