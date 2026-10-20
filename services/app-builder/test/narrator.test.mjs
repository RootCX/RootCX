import test from 'node:test';
import assert from 'node:assert/strict';
import { narrator, businessText, narrationGateway } from '../narrator.mjs';
import { narrationFact } from '../opencode.mjs';

const tick = () => new Promise(resolve => setImmediate(resolve));
const signal = () => new AbortController().signal;

test('slow narration never blocks observations or replaces a newer phase', async t => {
  let input; let release;
  const messages = [];
  const voice = narrator({ prompt: 'Suivre les visites', signal: signal(), onProgress: (...args) => messages.push(args),
    translate: async value => { input = value; return new Promise(resolve => { release = resolve; }); },
  });
  t.after(() => voice.close());
  voice.observe(narrationFact({ properties: { part: { type: 'tool', state: {
    input: { filePath: 'src/private-client.tsx' }, status: 'running', output: 'private credentials',
  } } } }, 'editing'));
  await tick();
  assert.ok(!JSON.stringify(input).includes('private'));
  assert.match(input.observation, /screen/);
  voice.observe({ phase: 'checking' });
  release('Je prépare le suivi des visites sur les fiches clients.'); await tick();
  assert.deepEqual(messages, []);
});

test('closing or cancelling narration discards pending translations', async () => {
  for (const ending of ['close', 'abort']) {
    let release;
    const controller = new AbortController();
    const messages = [];
    const voice = narrator({ prompt: 'Clients', signal: controller.signal, onProgress: text => messages.push(text),
      translate: () => new Promise(resolve => { release = resolve; }),
    });
    voice.observe({ phase: 'editing' }); await tick();
    if (ending === 'close') voice.close(); else controller.abort();
    release('Je prépare les fiches clients.'); await tick();
    voice.close();
    assert.deepEqual(messages, [], ending);
  }
});

test('technical, malformed, or oversized narration is discarded', () => {
  for (const value of [undefined, '', ' ', 'x'.repeat(501), 'Je modifie src/App.tsx', 'npm install', 'Le SQL est prêt', 'https://secret.invalid', '<script>']) assert.equal(businessText(value), null, String(value));
  assert.equal(businessText('Je prépare la date sur les fiches clients.'), 'Je prépare la date sur les fiches clients.');
});

test('the voice accepts one business sentence and cannot announce publication', async () => {
  for (const [text, delivered] of [
    ['Je prépare la date de visite sur la fiche client.', true],
    ['Je prépare la fiche. Je regarde la date.', false],
    ['Votre modification est disponible.', false],
    ['La modification est terminée.', false],
    ['Le changement est appliqué.', false],
    ['Je lance npm pour votre page.', false],
    ['RIEN', false],
  ]) {
    const messages = [];
    const voice = narrator({ prompt: 'Ajouter une date de visite', signal: signal(), translate: async () => text,
      onProgress: (...args) => messages.push(args) });
    try {
      voice.observe({ phase: 'editing', status: 'running' }); await tick();
      assert.equal(messages.length, delivered ? 1 : 0, text);
    } finally { voice.close(); }
  }
});

test('duplicate events and messages do not consume calls or repeat updates', async t => {
  let now = 0; let calls = 0;
  const messages = [];
  t.mock.method(Date, 'now', () => now);
  const voice = narrator({ prompt: 'Date de visite', signal: signal(), onProgress: (...args) => messages.push(args),
    translate: async () => { calls++; return 'Je prépare la date de visite.'; },
  });
  t.after(() => voice.close());
  voice.observe({ phase: 'editing', status: 'running' }); await tick();
  now += 4001;
  voice.observe({ phase: 'editing', status: 'running' }); await tick();
  assert.equal(calls, 1);
  voice.observe({ phase: 'editing', status: 'completed' }); await tick();
  assert.equal(calls, 2);
  assert.equal(messages.length, 1);
});

test('narration coalesces bursts and stops spending after its request budget', async t => {
  let now = 0; let calls = 0;
  t.mock.method(Date, 'now', () => now);
  const voice = narrator({ prompt: 'Clients', signal: signal(), onProgress() {}, translate: async () => { calls++; return null; } });
  t.after(() => voice.close());
  for (const phase of ['understanding', 'editing', 'checking', 'repairing', 'waiting']) {
    for (const status of ['pending', 'running', 'completed', 'error']) {
      for (const area of ['screen', 'data', 'application']) {
        voice.observe({ phase, status, area });
        await tick();
        now += 4001;
      }
    }
  }
  assert.equal(calls, 40);
});

test('authorization or credit refusal disables further narration', async t => {
  let now = 0;
  t.mock.method(Date, 'now', () => now);
  for (const status of [401, 402, 403, 404]) {
    let calls = 0;
    const voice = narrator({ prompt: 'Clients', signal: signal(), onProgress() {},
      translate: async () => { calls++; throw Object.assign(new Error('Unavailable'), { status }); },
    });
    voice.observe({ phase: 'editing' }); await tick(); now += 4001;
    voice.observe({ phase: 'checking' }); await tick(); voice.close();
    assert.equal(calls, 1, String(status));
  }
});

test('gateway never uses the coding endpoint even without narration configuration', async t => {
  const calls = [];
  t.mock.method(globalThis, 'fetch', async (url, options) => {
    calls.push({ url, body: JSON.parse(options.body) });
    return { ok: true, json: async () => ({ message: 'Je prépare votre page.' }) };
  });
  const input = { request: 'Bonjour', phase: 'editing', observation: '', previousMessage: '' };
  const config = { endpoint: 'https://coding.invalid', model: 'expensive', apiKey: 'test' };
  assert.equal(await narrationGateway(config)(input, signal()), null);
  assert.equal(calls.length, 0);
  const result = await narrationGateway({ ...config, narrationEndpoint: 'https://portal.invalid/api/llm/v1/narration' })(input, signal());
  assert.equal(result, 'Je prépare votre page.');
  assert.deepEqual(calls, [{ url: 'https://portal.invalid/api/llm/v1/narration', body: input }]);
});

test('a hung translator releases the slot at the deadline without blocking later facts', async t => {
  let now = 0; let calls = 0;
  const messages = [];
  t.mock.method(Date, 'now', () => now);
  const voice = narrator({ prompt: 'Page', signal: signal(), onProgress: (...args) => messages.push(args),
    translate: () => ++calls === 1 ? new Promise(() => {}) : Promise.resolve('Je vérifie la page.'),
  });
  t.after(() => voice.close());
  voice.observe({ phase: 'editing' });
  await new Promise(resolve => setTimeout(resolve, 3100));
  now = 4001;
  voice.observe({ phase: 'checking' }); await tick();
  assert.equal(calls, 2);
  assert.deepEqual(messages, [['checking', 'Je vérifie la page.']]);
});


test('distinct tools and returning phases remain observable without exposing tool identity', async t => {
  let now = 0;
  t.mock.method(Date, 'now', () => now);
  const inputs = [];
  const voice = narrator({ prompt: 'Acompte', signal: signal(), onProgress() {},
    translate: async input => { inputs.push(input); return null; },
  });
  t.after(() => voice.close());
  for (const fact of [
    { id: 'private-1', phase: 'checking', status: 'running', action: 'typecheck' },
    { id: 'private-2', phase: 'checking', status: 'running', action: 'test' },
    { id: 'private-3', phase: 'repairing', status: 'error', action: 'test' },
    { id: 'private-4', phase: 'checking', status: 'running', action: 'test' },
  ]) {
    voice.observe(fact); await tick(); now += 4001;
    voice.observe(fact); await tick();
  }
  assert.equal(inputs.length, 4);
  assert.deepEqual(inputs.map(input => JSON.parse(input.observation).action), ['typecheck', 'test', 'test', 'test']);
  assert.doesNotMatch(JSON.stringify(inputs), /private/);
});


test('a duplicate old tool cannot invalidate the current translation', async t => {
  let now = 0; let release;
  const messages = [];
  t.mock.method(Date, 'now', () => now);
  const voice = narrator({ prompt: 'Acompte', signal: signal(), onProgress: (...args) => messages.push(args),
    translate: input => input.phase === 'editing' ? Promise.resolve(null) : new Promise(resolve => { release = resolve; }),
  });
  t.after(() => voice.close());
  const old = { id: 'edit-1', phase: 'editing', status: 'running' };
  voice.observe(old); await tick(); now += 4001;
  voice.observe({ id: 'test-1', phase: 'checking', status: 'running', action: 'test' }); await tick();
  voice.observe(old);
  release('Je vérifie le calcul de votre acompte.'); await tick();
  assert.deepEqual(messages, [['checking', 'Je vérifie le calcul de votre acompte.']]);
});
