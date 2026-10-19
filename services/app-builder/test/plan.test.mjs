import test from 'node:test';
import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { mkdtemp, mkdir, readFile, rm, writeFile, access } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { collect } from '../workspace.mjs';
import { isolated } from '../sandbox.mjs';
import { coherenceGateway, settlePlan } from '../plan.mjs';

const cli = fileURLToPath(new URL('../node_modules/@fission-ai/openspec/bin/openspec.js', import.meta.url));
// The image runs the pinned CLI inside the publication sandbox, exactly as agent.mjs does.
const imaged = process.platform === 'linux' && await access('/usr/local/bin/openspec').then(() => true, () => false);

// Same contract as the sandboxed runner: resolves with output, rejects with the output as message.
const local = root => imaged ? (args, signal) => isolated(root, args, { signal }) : (args, signal) => new Promise((resolve, reject) => {
  execFile(process.execPath, [cli, ...args.slice(1)], { cwd: root, signal, env: { PATH: process.env.PATH, HOME: root, OPENSPEC_TELEMETRY: '0', DO_NOT_TRACK: '1' } },
    (error, stdout, stderr) => error ? reject(new Error(`${stdout}${stderr}`)) : resolve(stdout + stderr));
});

const deposit = `# devis Specification

## Purpose
Calculer, envoyer et suivre les devis de travaux du chiffrage jusqu'à l'acceptation par le client.

## Requirements

### Requirement: Acompte
L'acompte d'un devis DOIT être un pourcentage de son TTC.

#### Scenario: Acompte 30 %
- **WHEN** un devis de 10 000 € TTC demande 30 % d'acompte
- **THEN** la facture d'acompte vaut 3 000 € TTC
`;
const depositTest = `import { test } from 'node:test'\ntest('Acompte › Acompte 30 %', () => {})\n`;

async function workspace(t, files) {
  const root = await mkdtemp(join(tmpdir(), 'builder-plan-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  for (const [path, content] of Object.entries(files)) {
    await mkdir(dirname(join(root, path)), { recursive: true });
    await writeFile(join(root, path), content);
  }
  return root;
}

function change(name, delta, domain = 'devis') {
  return {
    [`openspec/changes/${name}/proposal.md`]: `## Why\nLe patron veut faire évoluer ses devis.\n\n## What Changes\n- ${name}\n`,
    [`openspec/changes/${name}/tasks.md`]: '## 1. Mise en œuvre\n- [x] 1.1 Fait\n',
    [`openspec/changes/${name}/specs/${domain}/spec.md`]: delta,
  };
}

const coherent = async () => [];
const base = { 'manifest.json': '{"appId":"demo"}', 'openspec/specs/devis/spec.md': deposit, 'backend/devis.test.ts': depositTest };

test('applications without a plan keep the existing publication path', async t => {
  const root = await workspace(t, { 'manifest.json': '{}', 'backend/index.ts': 'export {}' });
  const initial = await collect(root);
  await writeFile(join(root, 'backend/index.ts'), 'export const changed = true');
  const result = await settlePlan({ root, initial, collect, run: () => assert.fail('no CLI'), review: () => assert.fail('no review') });
  assert.equal(result.active, false);
});

test('a modified rule replaces the old one in place and its dated change stays as history', async t => {
  const root = await workspace(t, base);
  const initial = await collect(root);
  for (const [path, content] of Object.entries({
    ...change('plafonner-acompte', `## MODIFIED Requirements

### Requirement: Acompte
L'acompte d'un devis DOIT être un pourcentage de son TTC, plafonné à 30 %.

#### Scenario: Acompte 30 %
- **WHEN** un devis de 10 000 € TTC demande 30 % d'acompte
- **THEN** la facture d'acompte vaut 3 000 € TTC

#### Scenario: Acompte 50 % refusé
- **WHEN** un devis demande 50 % d'acompte
- **THEN** le devis est refusé
`),
    'backend/devis.test.ts': `${depositTest}test('Acompte › Acompte 50 % refusé', () => {})\n`,
    'backend/index.ts': 'export const maximum = 30',
  })) { await mkdir(dirname(join(root, path)), { recursive: true }); await writeFile(join(root, path), content); }
  const reviewed = [];
  const { files } = await settlePlan({ root, initial, collect, run: local(root), review: async plan => { reviewed.push(plan); return []; } });
  const plan = Buffer.from(files['openspec/specs/devis/spec.md'], 'base64').toString();
  assert.match(plan, /plafonné à 30 %/);
  assert.equal(plan.match(/### Requirement:/g).length, 1);
  assert.ok(Object.keys(files).some(path => /^openspec\/changes\/archive\/\d{4}-\d{2}-\d{2}-plafonner-acompte\/proposal\.md$/.test(path)), 'dated history kept');
  assert.ok(!Object.keys(files).some(path => path.startsWith('openspec/changes/plafonner-acompte/')), 'nothing pending');
  assert.equal(reviewed.length, 1);
  assert.match(reviewed[0], /openspec\/specs\/devis\/spec\.md/);
  assert.doesNotMatch(reviewed[0], /changes\/archive/, 'history is never reviewed as current behaviour');
});

test('a rule added twice under the same name is refused before anything changes', async t => {
  const root = await workspace(t, { ...base, ...change('doublon', `## ADDED Requirements

### Requirement: Acompte
L'acompte DOIT être un montant fixe de 500 €.

#### Scenario: Acompte fixe
- **WHEN** un devis est accepté
- **THEN** la facture d'acompte vaut 500 €
`) });
  const initial = await collect(root);
  await assert.rejects(settlePlan({ root, initial, collect, run: local(root), review: coherent }),
    error => error.repairable && /already exists/.test(error.message));
  assert.equal(await readFile(join(root, 'openspec/specs/devis/spec.md'), 'utf8'), deposit);
});

test('a reworded duplicate passes OpenSpec but the coherence review sends it back for repair', async t => {
  const root = await workspace(t, base);
  const initial = await collect(root);
  for (const [path, content] of Object.entries({
    ...change('acompte-client', `## ADDED Requirements

### Requirement: Acompte client
L'acompte DOIT être un montant fixe de 500 €.

#### Scenario: Acompte fixe
- **WHEN** un devis est accepté
- **THEN** la facture d'acompte vaut 500 €
`),
    'backend/devis.test.ts': `${depositTest}test('Acompte client › Acompte fixe', () => {})\n`,
  })) { await mkdir(dirname(join(root, path)), { recursive: true }); await writeFile(join(root, path), content); }
  const review = async plan => /Acompte client/.test(plan) ? [{ kind: 'contradiction', requirements: ['Acompte', 'Acompte client'], explanation: 'Deux montants d’acompte différents pour le même devis.' }] : [];
  await assert.rejects(settlePlan({ root, initial, collect, run: local(root), review }),
    error => error.repairable && /contradiction: Acompte \/ Acompte client/.test(error.message));
});

test('screen-only changes publish without touching the plan', async t => {
  const root = await workspace(t, { ...base, 'src/App.tsx': 'export const title = "Devis"' });
  const initial = await collect(root);
  await writeFile(join(root, 'src/App.tsx'), 'export const title = "Mes devis"');
  const result = await settlePlan({ root, initial, collect, run: local(root), review: coherent });
  assert.equal(result.active, true);
});

test('the coherence review reads the whole plan once per version and fails closed', async () => {
  const requests = [];
  const review = coherenceGateway({ endpoint: 'https://ai.example/v1/messages', apiKey: 'key', model: 'model' }, async (url, init) => {
    requests.push(JSON.parse(init.body));
    return new Response(JSON.stringify({ content: [{ type: 'tool_use', name: 'report_coherence', input: { issues: [{ kind: 'duplicate', requirements: ['A', 'B'], explanation: 'Même règle.' }] } }] }));
  });
  assert.deepEqual(await review('<plan/>'), [{ kind: 'duplicate', requirements: ['A', 'B'], explanation: 'Même règle.' }]);
  assert.deepEqual(requests[0].tool_choice, { type: 'auto' });
  assert.equal(requests[0].tools[0].strict, true);
  assert.equal(requests[0].temperature, undefined);
  for (const [response, code] of [[new Response('', { status: 402 }), 'AI_CREDITS_EXHAUSTED'], [new Response('{"content":[]}'), 'AI_UNAVAILABLE']]) {
    await assert.rejects(coherenceGateway({}, async () => response)('<plan/>'), error => error.fatal && error.code === code);
  }
});

test('the image pins the same OpenSpec release as the tests', async () => {
  const image = JSON.parse(await readFile(new URL('../tools/openspec/package.json', import.meta.url), 'utf8'));
  const local = JSON.parse(await readFile(new URL('../package.json', import.meta.url), 'utf8'));
  assert.equal(image.dependencies['@fission-ai/openspec'], local.devDependencies['@fission-ai/openspec']);
});
