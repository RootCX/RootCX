import test from 'node:test';
import assert from 'node:assert/strict';
import { narrator, businessText, narrationGateway } from '../narrator.mjs';
import { narrationFact, progressFor } from '../opencode.mjs';

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
    ['Je vérifie tout après la publication.', false],
    ['Le calcul a été testé avec succès.', false],
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


test('reading rules or tests does not announce preparation or verification', () => {
  for (const command of ['cat openspec/specs/devis/spec.md', 'cd /workspace && cat openspec/specs/devis/spec.md', 'openspec instructions explore --json', 'cat backend/pricing.test.ts', 'cd /workspace && cat backend/pricing.test.ts', 'cd /workspace && openspec instructions apply --change test-devis --json', 'rg test package.json', "sed -n '1,20p' backend/test.ts"]) {
    const event = { properties: { part: { type: 'tool', tool: 'shell', state: { status: 'running', input: { command } } } } };
    assert.equal(progressFor(event), 'understanding', command);
    assert.equal(narrationFact(event, progressFor(event)).action, 'inspect', command);
  }
});

test('same-phase outcome changes invalidate late success narration', async t => {
  let release;
  const delivered = [];
  const voice = narrator({ prompt: 'Acompte', signal: signal(), onProgress: (...args) => delivered.push(args), translate: () => new Promise(resolve => { release = resolve; }) });
  t.after(() => voice.close());
  voice.observe({ id: 'test', phase: 'checking', action: 'test', status: 'running', source: 'platform' }); await tick();
  voice.observe({ id: 'test', phase: 'checking', action: 'test', status: 'error', source: 'platform' });
  release('Je vérifie le calcul de l’acompte.'); await tick();
  assert.deepEqual(delivered, []);
});

test('verified narration requires a completed platform check, never an agent intention', async () => {
  for (const [source, status, action, accepted] of [['tool', 'completed', 'test', false], ['platform', 'running', 'test', false], ['platform', 'error', 'test', false], ['platform', 'completed', 'build', false], ['platform', 'completed', 'test', true]]) {
    const delivered = [];
    const voice = narrator({ prompt: 'Acompte', signal: signal(), onProgress: (...args) => delivered.push(args), translate: async () => ({ message: 'Les comportements contrôlés sont conformes.', kind: 'verified' }) });
    try {
      voice.observe({ phase: 'checking', source, status, action, intent: 'Tout est déjà vérifié et publié' }); await tick();
      assert.equal(delivered.length, accepted ? 1 : 0, `${source}/${status}/${action}`);
    } finally { voice.close(); }
  }
});

test('tool purpose is contextual intention, never raw command or output', () => {
  const event = { properties: { part: { type: 'tool', tool: 'shell', id: 'private-id', state: { status: 'completed', input: { description: 'Adapter l’acompte à 40 %', command: 'cat private.txt' }, output: 'secret diagnostic' } } } };
  const fact = narrationFact(event, progressFor(event));
  assert.equal(fact.intent, 'Adapter l’acompte à 40 %');
  assert.equal(fact.source, 'tool');
  assert.doesNotMatch(JSON.stringify(fact), /private.txt|diagnostic/);
});

test('prolonged running activity can reassure once without inventing completed work', async t => {
  let now = 0; let timer; const inputs = [];
  t.mock.method(Date, 'now', () => now);
  t.mock.method(globalThis, 'setInterval', fn => { timer = fn; return { unref() {} }; });
  t.mock.method(globalThis, 'clearInterval', () => {});
  const voice = narrator({ prompt: 'Devis', context: [{ user: 'Changer le taux', assistantPrepared: 'Lequel ?', publication: 'unknown' }], signal: signal(), onProgress() {}, translate: async input => { inputs.push(input); return null; } });
  t.after(() => voice.close());
  voice.observe({ id: 'long', phase: 'checking', action: 'test', status: 'running', source: 'platform' }); await tick();
  now = 21000; timer(); await tick();
  assert.equal(inputs.length, 2);
  assert.deepEqual(inputs[1].context[0].publication, 'unknown');
  assert.equal(JSON.parse(inputs[1].observation).stillRunning, true);
  assert.equal(JSON.parse(inputs[1].observation).status, 'running');
  voice.observe({ id: 'long', phase: 'checking', action: 'test', status: 'completed', source: 'platform' });
  now = 25001; timer(); await tick();
  now = 60000; timer(); await tick();
  assert.equal(inputs.length, 3, 'never imply work is continuing after completion');
});

test('batched outcomes stay chronological and a later failure invalidates earlier success', async t => {
  let now = 0; let timer; const inputs = []; const delivered = [];
  t.mock.method(Date, 'now', () => now);
  t.mock.method(globalThis, 'setInterval', fn => { timer = fn; return { unref() {} }; });
  t.mock.method(globalThis, 'clearInterval', () => {});
  const voice = narrator({ prompt: 'Acompte', signal: signal(), onProgress: (...args) => delivered.push(args), translate: async input => {
    inputs.push(input); return inputs.length === 1 ? null : { message: 'Les comportements contrôlés sont conformes.', kind: 'verified' };
  } });
  t.after(() => voice.close());
  voice.observe({ id: 'first', phase: 'understanding', action: 'inspect', status: 'running' }); await tick();
  voice.observe({ id: 'review', phase: 'checking', source: 'platform', action: 'review', status: 'running' });
  voice.observe({ id: 'test', phase: 'checking', source: 'platform', action: 'test', status: 'completed' });
  voice.observe({ id: 'review', phase: 'checking', source: 'platform', action: 'review', status: 'error' });
  now = 4001; timer(); await tick();
  const facts = inputs[1].observation.split('\n').map(line => JSON.parse(line));
  assert.deepEqual(facts.map(fact => fact.status), ['completed', 'error']);
  assert.deepEqual(delivered, []);
});
