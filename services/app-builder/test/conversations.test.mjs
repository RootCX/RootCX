import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { randomUUID } from 'node:crypto';
import { conversationSession } from '../opencode.mjs';

test('conversation contexts are isolated and reopened after reconnect', async t => {
  const home = await mkdtemp(join(tmpdir(), 'shappy-context-'));
  t.after(() => rm(home, { recursive: true, force: true }));
  const sessions = new Set();
  const api = async path => {
    if (path === '/session') { const id = randomUUID(); sessions.add(id); return { id }; }
    const id = path.split('/').at(-1);
    if (!sessions.has(id)) throw new Error('OpenCode request failed (404)');
    return { id };
  };
  const a = randomUUID(); const b = randomUUID();
  const first = await conversationSession(api, home, a);
  const second = await conversationSession(api, home, b);
  assert.notEqual(first, second);
  assert.equal(await conversationSession(api, home, a), first);
  await assert.rejects(conversationSession(async () => { throw new Error('OpenCode request failed (500)'); }, home, a), /500/, 'temporary errors must not silently erase conversation memory');
  sessions.delete(first);
  assert.notEqual(await conversationSession(api, home, a), first, 'a deleted native session can be recreated');
  assert.equal(await conversationSession(api, home, b), second);
  await assert.rejects(conversationSession(api, home, '../escape'), /Invalid conversation/);
});
