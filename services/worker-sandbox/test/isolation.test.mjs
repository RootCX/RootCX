import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, writeFile, rm } from 'node:fs/promises';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawn } from 'node:child_process';
import { createServer } from 'node:http';
import { createServer as netServer } from 'node:net';

const runtime = dirname(dirname(fileURLToPath(import.meta.url)));
// Core supplies HOME before Bun starts, including when its numeric UID has no passwd entry.
const launcherEnv = { PATH: process.env.PATH, HOME: '/tmp' };

test('Linux worker preserves IPC and nonce socket while denying parent files, processes and private network', { skip: process.platform !== 'linux', timeout: 40000 }, async t => {
  const parent = await mkdtemp('/tmp/core-worker-test-');
  const root = join(parent, 'app');
  await mkdir(root);
  const secret = join(parent, 'other-app-secret');
  await writeFile(secret, 'parent-database-secret');
  const prelude = join(parent, 'prelude.js');
  await writeFile(prelude, '');
  const storageSocket = join(parent, 'storage.sock');
  const hiddenSocket = join(parent, 'private.sock');
  const abstractSocket = '\0rootcx-qualification-' + process.pid;
  const storage = createServer((req, res) => { res.statusCode = new URL(req.url, 'http://storage').pathname === '/api/v1/storage/download/nonce' ? 200 : 404; res.end(res.statusCode === 200 ? 'nonce-file' : 'unavailable'); });
  const hidden = netServer(socket => socket.end('private'));
  const abstract = netServer(socket => socket.end('private'));
  for (const [server, path] of [[storage, storageSocket], [hidden, hiddenSocket], [abstract, abstractSocket]]) await new Promise(resolve => server.listen(path, resolve));
  t.after(async () => { storage.closeAllConnections(); await Promise.all([storage, hidden, abstract].map(server => new Promise(resolve => server.close(resolve)))); await rm(parent, { recursive: true, force: true }); });
  const source = `
    import assert from 'node:assert/strict';
    import fs from 'node:fs';
    import {connect} from 'node:net';
    assert.equal(process.env.CORE_DATABASE_SECRET,undefined);
    assert.equal(process.cwd(),${JSON.stringify(root)});
    assert.equal(fs.existsSync(${JSON.stringify(secret)}),false);
    assert.equal(fs.existsSync('/data/RootCX'),false);
    assert.equal(fs.existsSync('/var/run/secrets/kubernetes.io/serviceaccount/token'),false);
    assert.throws(()=>fs.writeFileSync(import.meta.filename,'tamper'));
    fs.writeFileSync(process.env.HOME+'/working','ok');
    for(const pid of fs.readdirSync('/proc').filter(p=>/^\\d+$/.test(p))){try{assert.equal(fs.readFileSync('/proc/'+pid+'/environ').includes('CORE_DATABASE_SECRET'),false)}catch(e){if(e.code!== 'ENOENT' && e.code!=='EACCES')throw e}}
    for(const path of [${JSON.stringify(hiddenSocket)},${JSON.stringify(abstractSocket)}]){
      await new Promise((resolve,reject)=>{const socket=connect(path);socket.once('connect',()=>{socket.destroy();reject(Error('hostsocket visible'))});socket.once('error',resolve)});
    }
    const r=await fetch('http://storage/api/v1/storage/download/nonce',{unix:process.env.ROOTCX_WORKER_STORAGE_SOCKET});assert.equal(await r.text(),'nonce-file');
    assert.equal((await fetch('http://storage/health',{unix:process.env.ROOTCX_WORKER_STORAGE_SOCKET})).status,404);
    for(const url of ['http://127.0.0.1:9100/health','http://169.254.169.254/latest/meta-data/','https://10.0.0.1/','https://localhost/']){
      try{const r=await fetch(url,{signal:AbortSignal.timeout(1500)});assert.equal(r.status,403)}catch(e){if(e.code==='ERR_ASSERTION')throw e}
    }
    if(process.env.TEST_PUBLIC==='1'){const r=await fetch('https://registry.npmjs.org/-/ping',{signal:AbortSignal.timeout(15000)});assert.equal(r.ok,true)}
    for await(const chunk of process.stdin){process.stdout.write(JSON.stringify({ipc:chunk.toString(),isolated:true})+'\\n');break}
  `;
  const entry = join(root, 'worker.mjs');
  await writeFile(entry, source);
  const bun = process.env.BUN_PATH || process.execPath;
  const spec = { root, prelude, args: [bun, '--preload', prelude, entry], storageSocket, env: { TEST_PUBLIC: process.env.ROOTCX_TEST_PUBLIC_NETWORK || '', HTTP_PROXY: 'http://169.254.169.254:80', HOME: '/data' } };
  const child = spawn(bun, [join(runtime, 'supervisor.mjs'), JSON.stringify(spec)], { cwd: runtime, env: { ...launcherEnv, CORE_DATABASE_SECRET: 'should-not-be-in-worker', HTTP_PROXY: 'http://169.254.169.254:80' }, stdio: ['pipe', 'pipe', 'pipe'] });
  t.after(() => child.kill('SIGKILL'));
  let stdout = '', stderr = '';
  child.stdout.on('data', data => { stdout += data; });
  child.stderr.on('data', data => { stderr += data; });
  child.stdin.end('request');
  const code = await new Promise((resolve, reject) => { child.once('error', reject); child.once('exit', resolve); });
  assert.equal(code, 0, stderr);
  assert.deepEqual(JSON.parse(stdout), { ipc: 'request', isolated: true });
});

test('dependency installation is confined and never runs package lifecycle scripts', { skip: process.platform !== 'linux' || process.env.ROOTCX_TEST_PUBLIC_NETWORK !== '1', timeout: 40000 }, async t => {
  const root = await mkdtemp('/tmp/core-dependency-test-');
  t.after(() => rm(root, { recursive: true, force: true }));
  await writeFile(join(root, 'package.json'), JSON.stringify({
    name: 'sandbox-install-proof', private: true,
    dependencies: { 'is-number': '7.0.0' },
    scripts: { preinstall: 'touch forbidden-script-ran', install: 'touch forbidden-script-ran', postinstall: 'touch forbidden-script-ran' },
  }));
  const bun = process.env.BUN_PATH || process.execPath;
  const spec = { root, args: [bun, 'install', '--ignore-scripts'], env: {}, writeRoot: true, timeoutMs: 30000 };
  const child = spawn(bun, [join(runtime, 'supervisor.mjs'), JSON.stringify(spec)], { cwd: runtime, env: launcherEnv, stdio: ['ignore', 'pipe', 'pipe'] });
  t.after(() => child.kill('SIGKILL'));
  let stderr = '';
  child.stdout.resume(); child.stderr.on('data', data => { stderr += data; });
  const code = await new Promise((resolve, reject) => { child.once('error', reject); child.once('exit', resolve); });
  assert.equal(code, 0, stderr);
  const { existsSync, readFileSync } = await import('node:fs');
  assert.equal(existsSync(join(root, 'forbidden-script-ran')), false);
  assert.equal(JSON.parse(readFileSync(join(root, 'node_modules/is-number/package.json'))).version, '7.0.0');
});
