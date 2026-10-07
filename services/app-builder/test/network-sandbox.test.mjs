import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { createServer as unixServer } from 'node:net';
import { mkdtemp, mkdir, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSandbox } from '../sandbox.mjs';
import { sandboxPolicy } from '../sandbox-policy.mjs';

test('model bridge never widens the sandbox to arbitrary hosts or ports', () => {
  for (const providerUrl of ['https://127.0.0.1:1234', 'http://localhost:1234', 'http://127.0.0.1', 'http://169.254.169.254:80', 'http://user@127.0.0.1:1234']) {
    assert.throws(() => sandboxPolicy({ root: '/app', home: '/state', readPaths: [], providerUrl }), /explicit loopback port/, providerUrl);
  }
});

test('agent sandbox reaches only its model bridge and cannot read runner or sibling state', { skip: process.platform !== 'linux', timeout: 30_000 }, async t => {
  const directory = await mkdtemp(join(tmpdir(), 'network-sandbox-'));
  const root = join(directory, 'app');
  const sibling = join(directory, 'sibling');
  await mkdir(root); await mkdir(sibling);
  await writeFile(join(sibling, 'secret'), 'client-secret');
  const allowed = createServer((req, res) => res.end('model-bridge'));
  const denied = createServer((req, res) => res.end('private-service'));
  const socket = unixServer(connection => connection.end('private-socket'));
  // Even an accidentally exposed socket must not grant access to its service.
  const socketPath = join(root, 'private.sock');
  await Promise.all([
    new Promise(resolve => allowed.listen(0, '127.0.0.1', resolve)),
    new Promise(resolve => denied.listen(0, '127.0.0.1', resolve)),
    new Promise(resolve => socket.listen(socketPath, resolve)),
  ]);
  t.after(async () => {
    allowed.closeAllConnections(); denied.closeAllConnections();
    allowed.close(); denied.close(); socket.close();
    await rm(directory, { recursive: true, force: true });
  });
  const providerUrl = `http://127.0.0.1:${allowed.address().port}`;
  const deniedUrl = `http://127.0.0.1:${denied.address().port}`;
  process.env.ROOTCX_TEST_SECRET = 'runner-secret';
  const child = await spawnSandbox(root, ['bun', '-e', `
    const fs = require('node:fs'); const net = require('node:net');
    const assert = require('node:assert/strict');
    assert.equal(process.env.ROOTCX_TEST_SECRET, undefined);
    assert.equal(fs.existsSync(${JSON.stringify(join(sibling, 'secret'))}), false);
    assert.equal(fs.existsSync('/var/run/secrets/kubernetes.io/serviceaccount/token'), false);
    assert.equal(fs.existsSync(${JSON.stringify(socketPath)}), true);
    for (const name of ['.agents', '.claude', '.config']) {
      assert.throws(() => fs.mkdirSync(process.env.HOME+'/'+name+'/plugins', {recursive:true}));
    }
    for (const pid of fs.readdirSync('/proc').filter(p => /^\\d+$/.test(p))) {
      try { assert.equal(fs.readFileSync('/proc/'+pid+'/environ').includes('runner-secret'), false); } catch (e) { if (e.code !== 'ENOENT' && e.code !== 'EACCES') throw e; }
    }
    assert.equal(await (await fetch(${JSON.stringify(providerUrl)})).text(), 'model-bridge');
    assert.equal((await fetch(${JSON.stringify(deniedUrl)})).status, 403);
    for (const url of ['https://example.com', 'http://169.254.169.254/latest/meta-data', 'https://rootcx.com/docs/developers/sdk.md']) {
      try { const response = await fetch(url, { signal: AbortSignal.timeout(1500) }); assert.equal(response.status, 403); }
      catch (error) { if (error.code === 'ERR_ASSERTION') throw error; }
    }
    for (const target of [{host:'127.0.0.1',port:${denied.address().port}}, {host:'169.254.169.254',port:80}, {host:'10.0.0.1',port:443}, {host:'1.1.1.1',port:443}, {path:${JSON.stringify(socketPath)}}]) {
      await new Promise((resolve,reject) => { const socket=net.connect(target); socket.on('connect',()=>{socket.destroy();reject(new Error('direct network escaped'))}); socket.on('error',resolve); socket.setTimeout(1000,()=>{socket.destroy();resolve()}); });
    }
    console.log('isolated model access');
  `], { providerUrl });
  t.after(() => { try { process.kill(-child.pid, 'SIGKILL'); } catch {} });
  let output = ''; let errors = '';
  child.stdout.on('data', bytes => { output += bytes; });
  child.stderr.on('data', bytes => { errors += bytes; });
  const status = await new Promise((resolve, reject) => { child.once('error', reject); child.once('close', resolve); });
  assert.equal(status, 0, errors);
  assert.match(output, /isolated model access/);
});

test('npm and Bun download through the registry allowlist', { skip: process.platform !== 'linux' || process.env.ROOTCX_SANDBOX_NETWORK_TESTS !== 'true', timeout: 60_000 }, async t => {
  const root = await mkdtemp(join(tmpdir(), 'registry-sandbox-'));
  t.after(async () => { await rm(root, { recursive: true, force: true }); await rm(`${root}.sandbox`, { recursive: true, force: true }); });
  await writeFile(join(root, 'package.json'), JSON.stringify({ name: 'sandbox-registry-check', private: true, dependencies: { 'is-number': '7.0.0' } }));
  const child = await spawnSandbox(root, ['bash', '-c', 'npm install --package-lock-only --ignore-scripts --no-audit --no-fund && npm ci --ignore-scripts --no-audit --no-fund && bun install --ignore-scripts && bun install --frozen-lockfile --ignore-scripts && node -e \'console.log("registry-proof=" + require("is-number")(42))\'']);
  t.after(() => { try { process.kill(-child.pid, 'SIGKILL'); } catch {} });
  let output = ''; let errors = '';
  child.stdout.on('data', bytes => { output += bytes; });
  child.stderr.on('data', bytes => { errors += bytes; });
  const status = await new Promise((resolve, reject) => { child.once('error', reject); child.once('close', resolve); });
  assert.equal(status, 0, errors);
  assert.match(output, /registry-proof=true/);
});
