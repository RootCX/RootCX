import test from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { get } from 'node:https';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { serviceServer } from '../service-server.mjs';

test('service TLS fails closed on partial configuration and reloads rotated certificates', async t => {
  const directory = await mkdtemp(join(tmpdir(), 'builder-tls-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const certFile = join(directory, 'tls.crt'), keyFile = join(directory, 'tls.key');
  for (const tls of [{ certFile }, { keyFile }]) assert.throws(() => serviceServer(() => {}, tls), /Both Builder TLS/);
  const certificate = () => execFileSync('openssl', ['req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1', '-subj', '/CN=localhost', '-addext', 'subjectAltName=IP:127.0.0.1', '-keyout', keyFile, '-out', certFile], { stdio: 'ignore' });
  certificate();
  t.mock.timers.enable({ apis: ['setInterval'] });
  const server = serviceServer((req, res) => res.end('protected'), { certFile, keyFile });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  t.after(() => { server.closeAllConnections(); server.close(); });
  const request = ca => new Promise((resolve, reject) => {
    get(`https://127.0.0.1:${server.address().port}`, { ca, agent: false }, res => {
      let body = ''; res.on('data', chunk => { body += chunk; }); res.on('end', () => resolve(body));
    }).on('error', reject);
  });
  assert.equal(await request(await readFile(certFile)), 'protected');
  await assert.rejects(request(undefined), /self-signed certificate/);
  certificate();
  const rotated = await readFile(certFile);
  await assert.rejects(request(rotated), /self-signed certificate/);
  t.mock.timers.tick(60_000);
  assert.equal(await request(rotated), 'protected');
});
