import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm, symlink, mkdir, readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { sourcePath, confined, materialize, collect, edit, validateFiles } from '../workspace.mjs';
import { progressFor, providerError } from '../opencode.mjs';
import { createBuilder } from '../server.mjs';
import { businessReply } from '../agent.mjs';

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

test('streaming failures preserve progress and a safe error code', async t => {
  const server = createBuilder({ token: 'b'.repeat(32), concurrency: 1 }, async ({ onProgress }) => {
    onProgress('editing');
    throw Object.assign(new Error('private provider credentials and output'), { code: 'AI_CREDITS_EXHAUSTED' });
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  t.after(() => { server.closeAllConnections(); server.close(); });
  const response = await fetch(`http://127.0.0.1:${server.address().port}/build`, {
    method: 'POST', headers: { authorization: `Bearer ${'b'.repeat(32)}`, accept: 'application/x-ndjson' },
    body: JSON.stringify({ appId: 'stream_test', prompt: 'Add a field', files: { 'manifest.json': encoded('{}') } }),
  });
  const lines = (await response.text()).trim().split('\n').map(JSON.parse);
  assert.deepEqual(lines, [{ type: 'progress', phase: 'understanding' }, { type: 'progress', phase: 'editing' }, { type: 'error', error: 'AI_CREDITS_EXHAUSTED' }]);
});

test('only activity categories leave the agent, never code or reasoning', () => {
  for (const type of ['text', 'reasoning']) assert.equal(progressFor({ type: 'message.part.updated', properties: { part: { type, text: 'secret SQL' } } }), null);
  for (const [tool, status, expected] of [['write', 'running', 'editing'], ['bash', 'running', 'editing'], ['read', 'completed', 'understanding'], ['bash', 'error', 'repairing']]) {
    assert.equal(progressFor({ type: 'message.part.updated', properties: { part: { type: 'tool', tool, state: { status, input: { command: 'secret' } } } } }), expected);
  }
  for (const [status, expected] of [[402, 'AI_CREDITS_EXHAUSTED'], [401, 'AI_CONFIGURATION'], [429, 'AI_UNAVAILABLE'], [503, 'AI_UNAVAILABLE'], [400, 'BUILD_FAILED']]) assert.equal(providerError({ data: { statusCode: status } }), expected);
});

test('conversational replies remain business language', () => {
  assert.equal(businessReply('Bonjour ! Que souhaitez-vous améliorer ?'), 'Bonjour ! Que souhaitez-vous améliorer ?');
  for (const text of ['Run npm install', 'Le frontend utilise TypeScript', 'Voir src/App.tsx', '```sql\nDROP TABLE clients;\n```']) {
    assert.equal(businessReply(text), 'Je suis là pour vous aider à faire évoluer votre application. Dites-moi ce que vous souhaitez changer.');
  }
});
