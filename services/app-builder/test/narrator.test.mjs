import test from 'node:test';
import assert from 'node:assert/strict';
import { narrator, businessText } from '../narrator.mjs';

const tick = () => new Promise(resolve => setImmediate(resolve));
test('narration is optional, bounded, and never leaks tool output or reasoning', async t => {
  let input; let release;
  t.mock.method(globalThis, 'fetch', async (_, options) => {
    input = JSON.parse(options.body);
    await new Promise(resolve => { release = resolve; });
    return { ok: true, json: async () => ({ content: [{ type: 'text', text: 'Je prépare le suivi des visites sur les fiches clients.' }] }) };
  });
  const messages = [];
  const voice = narrator({ config: { endpoint: 'https://example.invalid/messages', apiKey: 'test', model: 'test' }, prompt: 'Suivre les visites', signal: new AbortController().signal, onProgress: (...args) => messages.push(args) });
  t.after(() => voice.close());
  voice.observe({ properties: { part: { type: 'reasoning', text: 'private reasoning' } } });
  voice.observe({ properties: { part: { type: 'tool', state: { output: 'private credentials' } } } }, 'editing');
  await tick();
  assert.ok(!JSON.stringify(input).includes('private'));
  assert.equal(messages.length, 0, 'coding continues while narration is still pending');
  voice.observe({}, 'checking');
  release(); await tick();
  assert.equal(messages.length, 0, 'a late editing message cannot replace a newer checking phase');
});

test('closed narration cannot publish a late message', async t => {
  let release;
  t.mock.method(globalThis, 'fetch', async () => {
    await new Promise(resolve => { release = resolve; });
    return { ok: true, json: async () => ({ content: [{ type: 'text', text: 'Je prépare les fiches clients.' }] }) };
  });
  const messages = [];
  const voice = narrator({ config: {}, prompt: 'Clients', signal: new AbortController().signal, onProgress: (...args) => messages.push(args) });
  voice.observe({}, 'editing'); await tick(); voice.close(); release(); await tick();
  assert.deepEqual(messages, []);
});

test('technical, malformed, or oversized narration is discarded', () => {
  for (const value of [undefined, '', ' ', 'x'.repeat(501), 'Je modifie src/App.tsx', 'npm install', 'Le SQL est prêt', 'https://secret.invalid', '<script>']) assert.equal(businessText(value), null, String(value));
  assert.equal(businessText('Je prépare la date sur les fiches clients.'), 'Je prépare la date sur les fiches clients.');
});

test('the business voice delivers at most two sentences and cannot announce publication', async t => {
  for (const [text, delivered] of [
    ['Je prépare la fiche client. Je vérifie la place de la date de visite.', true],
    ['Je prépare la fiche. Je regarde la date. Je regarde la liste.', false],
    ['Votre modification est disponible.', false],
    ['La modification est terminée.', false],
    ['Le changement est appliqué.', false],
    ['Je lance npm pour votre page.', false],
    ['RIEN', false],
  ]) {
    const messages = [];
    const fetch = t.mock.method(globalThis, 'fetch', async () => ({
      ok: true, json: async () => ({ content: [{ type: 'text', text }] }),
    }));
    const voice = narrator({ config: {}, prompt: 'Ajouter une date de visite', signal: new AbortController().signal, onProgress: (...args) => messages.push(args) });
    try {
      voice.observe({ properties: { part: { type: 'tool', tool: 'edit', state: { status: 'running', output: 'private' } } } }, 'editing');
      await tick();
      assert.equal(messages.length, delivered ? 1 : 0, text);
    } finally { voice.close(); fetch.mock.restore(); }
  }
});

test('rereading during an edit keeps narration on the public phase', async t => {
  let now = 10_000;
  const messages = [];
  const inputs = [];
  t.mock.method(Date, 'now', () => now);
  t.mock.method(globalThis, 'fetch', async (_, options) => {
    inputs.push(JSON.parse(JSON.parse(options.body).messages[0].content));
    const text = inputs.length === 1 ? 'Je prépare la date de visite.' : 'Je regarde aussi la présentation de la fiche client.';
    return { ok: true, json: async () => ({ content: [{ type: 'text', text }] }) };
  });
  const voice = narrator({ config: {}, prompt: 'Ajouter une date de visite', signal: new AbortController().signal, onProgress: (...args) => messages.push(args) });
  t.after(() => voice.close());
  voice.observe({ properties: { part: { type: 'tool', state: { status: 'completed' } } } }, 'editing');
  await tick();
  now += 8001;
  voice.observe({ properties: { part: { type: 'tool', state: { status: 'completed' } } } }, 'understanding');
  await tick();
  assert.equal(messages.length, 2);
  assert.equal(messages[1][0], 'editing', 'the public stream must accept the new business update');
  assert.match(inputs[1].observation, /understanding/, 'the narrator still receives the actual observed activity');
});
