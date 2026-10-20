import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm, writeFile, readdir, mkdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { conversationContext, contextText } from '../narration-context.mjs';

test('history isolates sessions, survives source replacement and upserts repaired runs', async t => {
  const parent = await mkdtemp(join(tmpdir(), 'nova-context-'));
  t.after(() => rm(parent, { recursive: true, force: true }));
  const root = join(parent, 'application'); await mkdir(root);
  const first = await conversationContext(root, 'session-one', 'run-one', 'Change le taux');
  await first.reply('Quel pourcentage ?');
  await rm(root, { recursive: true }); await mkdir(root);
  const second = await conversationContext(root, 'session-one', 'run-two', '40 %');
  assert.deepEqual(second.previous, [{ user: 'Change le taux', assistantPrepared: 'Quel pourcentage ?', publication: 'unknown' }]);
  await second.reply('L’acompte est de 40 %.');
  const repair = await conversationContext(root, 'session-one', 'run-two', '40 %');
  assert.deepEqual(repair.previous, second.previous, 'repair must not repeat the current turn or trust its prepared result');
  await repair.reply('L’acompte sera de 40 %.');
  const next = await conversationContext(root, 'session-one', 'run-three', 'Merci');
  assert.equal(next.previous.length, 2);
  assert.equal(next.previous[1].assistantPrepared, 'L’acompte sera de 40 %.');
  assert.deepEqual((await conversationContext(root, 'session-two', 'other', 'Bonjour')).previous, []);
  assert.deepEqual(await readdir(root), [], 'conversation memory never enters application sources');
});

test('context remains bounded and unavailable memory never blocks execution', async t => {
  const parent = await mkdtemp(join(tmpdir(), 'nova-context-'));
  t.after(() => rm(parent, { recursive: true, force: true }));
  const root = join(parent, 'app');
  for (let i = 0; i < 8; i++) {
    const context = await conversationContext(root, 'session', String(i), 'u'.repeat(4000));
    assert.ok(context.previous.length <= 3);
    await context.reply('a'.repeat(4000));
  }
  const context = await conversationContext(root, 'session', 'last', 'Fin');
  assert.ok(context.previous.every(turn => turn.user.length <= 800 && turn.assistantPrepared.length <= 800));
  const [file] = await readdir(root + '.narration');
  await writeFile(join(root + '.narration', file), '{corrupt');
  assert.deepEqual((await conversationContext(root, 'session', 'new', 'Bonjour')).previous, []);
  await writeFile(join(parent, 'blocked.narration'), 'not a directory');
  const unavailable = await conversationContext(join(parent, 'blocked'), 'session', 'run', 'Bonjour');
  await unavailable.reply('Bonjour');
  assert.deepEqual(unavailable.previous, []);
});

test('conversation and intentions redact credentials, contact details and fenced code', () => {
  const text = contextText('Contacter alice@example.com https://private.invalid?token=secret token=abcdef password: xyz ```const privateKey = "secret"``` Bearer abcdefgh');
  assert.doesNotMatch(text, /alice|private.invalid|abcdef|xyz|privateKey|abcdefgh/);
});
