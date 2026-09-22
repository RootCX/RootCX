import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm, symlink, mkdir, readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { sourcePath, confined, materialize, collect, edit, validateFiles } from '../workspace.mjs';
import { code } from '../agent.mjs';
import { createBuilder } from '../server.mjs';

const encoded = text => Buffer.from(text).toString('base64');

test('agent file tools reject traversal, secrets and build-created links', async t => {
  const root = await mkdtemp(join(tmpdir(), 'builder-test-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  for (const path of ['../escape', '/etc/passwd', 'a/../b', '.git/config', 'a/.env', 'node_modules/a', '.npmrc', 'a\\b']) {
    assert.throws(() => sourcePath(path), path);
  }
  await mkdir(join(root, 'src'));
  await symlink(tmpdir(), join(root, 'src', 'escape'));
  await assert.rejects(confined(root, 'src/escape/secret'), /Links/);
  await assert.rejects(edit(root, 'src/escape/secret', 'bad'), /Links/);
});

test('final build validates edits made after an earlier successful check', async t => {
  const root = await mkdtemp(join(tmpdir(), 'builder-test-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  await materialize(root, { 'src/app.ts': encoded('original') });
  let turn = 0; const builds = [];
  const responses = [
    [{ type: 'tool_use', id: 'a', name: 'check', input: {} }],
    [{ type: 'tool_use', id: 'b', name: 'write_file', input: { path: 'src/app.ts', content: 'changed' } }],
    [{ type: 'text', text: 'Le champ a été ajouté.' }],
  ];
  const result = await code({ root, appId: 'example', prompt: 'Add a field', signal: new AbortController().signal,
    config: { endpoint: 'https://example.test/messages', apiKey: 'test', model: 'fixture' },
    request: async () => new Response(JSON.stringify({ content: responses[turn++], stop_reason: 'end_turn' })),
    compile: async () => { builds.push(await readFile(join(root, 'src/app.ts'), 'utf8')); return encoded('artifact'); },
  });
  assert.deepEqual(builds, ['original', 'changed']);
  assert.equal(result.files['src/app.ts'], encoded('changed'));
});

test('model cannot turn a failed build into a successful result', async t => {
  const root = await mkdtemp(join(tmpdir(), 'builder-test-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  await materialize(root, { 'src/app.ts': encoded('original') });
  await assert.rejects(code({ root, appId: 'example', prompt: 'Change it', config: {},
    request: async () => new Response(JSON.stringify({ content: [{ type: 'text', text: 'Done!' }] })),
    compile: async () => { throw new Error('Typecheck failed'); },
  }), /Typecheck failed/);
});

test('HTTP builder rejects unauthenticated execution and scopes each workspace', async t => {
  const server = createBuilder({ token: 'a'.repeat(32), concurrency: 1 }, async ({ root }) => ({ files: await collect(root), frontend: 'artifact', summary: 'Prepared' }));
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  t.after(() => { server.closeAllConnections(); server.close(); });
  const url = `http://127.0.0.1:${server.address().port}/build`;
  assert.equal((await fetch(url, { method: 'POST', body: '{}' })).status, 401);
  const response = await fetch(url, { method: 'POST', headers: { authorization: `Bearer ${'a'.repeat(32)}` },
    body: JSON.stringify({ appId: 'example', prompt: 'Change it', files: { 'manifest.json': encoded('{}') } }),
  });
  assert.equal(response.status, 200);
  assert.equal((await response.json()).files['manifest.json'], encoded('{}'));
});

test('binary source assets validate without regex stack overflow', () => {
  validateFiles({ 'public/model.glb': Buffer.alloc(4 * 1024 * 1024, 7).toString('base64') });
  for (const encoded of ['abc', '%%%%', 'YQ==ignored']) assert.throws(() => validateFiles({ 'file': encoded }));
});

test('insufficient AI credits reach Core as a payment failure without building or publishing', async t => {
  let builds = 0;
  const server = createBuilder({ token: 'b'.repeat(32), concurrency: 1 }, options => code({
    ...options, config: {},
    request: async () => new Response('{}', { status: 402 }),
    compile: async () => { builds++; return 'artifact'; },
  }));
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  t.after(() => { server.closeAllConnections(); server.close(); });
  const response = await fetch(`http://127.0.0.1:${server.address().port}/build`, {
    method: 'POST', headers: { authorization: `Bearer ${'b'.repeat(32)}` },
    body: JSON.stringify({ appId: 'example', prompt: 'Add a field', files: { 'manifest.json': encoded('{}') } }),
  });
  assert.equal(response.status, 402);
  assert.deepEqual(await response.json(), { error: 'AI_CREDITS_EXHAUSTED' });
  assert.equal(builds, 0);
});
