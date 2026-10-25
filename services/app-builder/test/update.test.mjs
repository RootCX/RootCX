import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { randomUUID } from 'node:crypto';
import { collect, materialize } from '../workspace.mjs';
import { createBuilder } from '../server.mjs';
import { applyUpdate, fetchPlan, replacementRequest } from '../update.mjs';

const encoded = text => Buffer.from(text).toString('base64');
const decoded = files => Object.fromEntries(Object.entries(files).map(([path, content]) => [path, Buffer.from(content, 'base64').toString()]));
const tree = files => Object.fromEntries(Object.entries(files).map(([path, text]) => [path, encoded(text)]));
const sidebar = label => `export const items = [\n  'Accueil',\n  '${label}',\n  'Clients',\n];\n`;
const base = { 'src/sidebar.ts': sidebar('Chantiers'), 'src/theme.ts': 'export const color = "blue";\n', 'src/legacy.ts': 'export const old = 1;\n' };

async function portal(t, answer) {
  const seen = { plans: [], reports: [] };
  const server = createServer(async (req, res) => {
    let body = '';
    for await (const chunk of req) body += chunk;
    const url = new URL(req.url, 'http://portal');
    if (req.method === 'POST') { seen.reports.push({ key: req.headers['x-api-key'], ...JSON.parse(body) }); res.writeHead(204).end(); return; }
    seen.plans.push({ key: req.headers['x-api-key'], appId: url.searchParams.get('appId'), conversationId: url.searchParams.get('conversationId') });
    const { status, plan } = answer();
    res.writeHead(status, { 'content-type': 'application/json' }).end(plan ? JSON.stringify(plan) : undefined);
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  t.after(() => { server.closeAllConnections(); server.close(); });
  return { seen, config: { apiKey: 'rk_tenant', updatesEndpoint: `http://127.0.0.1:${server.address().port}/api/builder/v1/updates` } };
}

function plan(mode, theirs) {
  return { mode, base: { version: '0.3.0', files: tree(base) }, release: { version: '0.4.1', files: tree(theirs) } };
}

async function update(t, { mode, ours, theirs, compile = async () => 'artifact', coding, prompt = 'Mettre à jour' }) {
  const root = await mkdtemp(join(tmpdir(), 'builder-update-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  await materialize(root, tree(ours));
  const { config, seen } = await portal(t, () => ({ status: 500 }));
  const conversationId = randomUUID();
  const run = applyUpdate({ plan: plan(mode, theirs), root, appId: 'btp', conversationId, prompt, signal: new AbortController().signal, config, compile, coding });
  return { run, root, seen, conversationId };
}

test('explicit replacement publishes exactly the requested release after verification, without the model', async t => {
  const ours = { ...base, 'src/private-feature.ts': 'company customization' };
  const theirs = { 'src/new-app.ts': 'new application' };
  const { run, root, seen } = await update(t, {
    mode: 'reconcile', ours, theirs, prompt: replacementRequest('0.4.1'),
    compile: async dir => { assert.deepEqual(decoded(await collect(dir)), theirs); return 'verified'; },
    coding: () => assert.fail('replacement must not regenerate the release'),
  });
  const result = await run;
  assert.deepEqual(decoded(result.files), theirs);
  assert.deepEqual(decoded(await collect(root)), theirs);
  assert.equal(result.frontend, 'verified');
  assert.match(result.summary, /personnalisations.*abandonnées/);
  assert.deepEqual(seen.reports, []);
});

test('replacement requires the exact target version and the authorized reconciliation state', async t => {
  for (const [mode, prompt] of [
    ['merge', replacementRequest('0.4.1')],
    ['reconcile', replacementRequest('0.3.0')],
    ['reconcile', `Ne pas ${replacementRequest('0.4.1')}`],
  ]) {
    const ours = { ...base, 'src/sidebar.ts': sidebar('Company') };
    const theirs = { ...base, 'src/sidebar.ts': sidebar('Release') };
    const { run } = await update(t, { mode, prompt, ours, theirs,
      compile: () => assert.fail('no replacement build'),
      coding: async () => ({ files: tree(ours), frontend: 'kept', summary: 'Ordinary reconciliation' }),
    });
    assert.deepEqual((await run).files, tree(ours), `${mode}: ${prompt}`);
  }
});

test('failed replacement verification reports failure and returns no publication', async t => {
  const { run, seen } = await update(t, {
    mode: 'reconcile', ours: base, theirs: { 'src/new.ts': 'invalid' },
    prompt: replacementRequest('0.4.1'), compile: async () => { throw new Error('compile failed'); },
    coding: () => assert.fail('no model'),
  });
  await assert.rejects(run, error => error.code === 'BUILD_FAILED');
  assert.deepEqual(seen.reports.map(report => report.outcome), ['failed']);
});

test('no update plan leaves the ordinary coding path untouched', async t => {
  for (const [name, endpoint, conversationId, expectedQueries] of [
    ['endpoint unset', false, randomUUID(), 0],
    ['conversation absent', true, null, 0],
    ['portal answers 204', true, randomUUID(), 1],
  ]) {
    const { config, seen } = await portal(t, () => ({ status: 204 }));
    const calls = [];
    const server = createBuilder({ token: 'u'.repeat(32), concurrency: 1, ...config, updatesEndpoint: endpoint ? config.updatesEndpoint : undefined }, async ({ root, prompt }) => {
      calls.push(prompt);
      return { files: await collect(root), frontend: '', summary: 'Prepared' };
    });
    await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
    t.after(() => { server.closeAllConnections(); server.close(); });
    const response = await fetch(`http://127.0.0.1:${server.address().port}/build`, {
      method: 'POST', headers: { authorization: `Bearer ${'u'.repeat(32)}` },
      body: JSON.stringify({ appId: 'btp', prompt: 'Ajouter un champ', conversationId, files: tree({ 'manifest.json': '{}' }) }),
    });
    assert.equal(response.status, 200, name);
    assert.deepEqual(calls, ['Ajouter un champ'], name);
    assert.equal(seen.plans.length, expectedQueries, name);
    if (expectedQueries) assert.deepEqual(seen.plans[0], { key: 'rk_tenant', appId: 'btp', conversationId }, name);
  }
});

test('an update conversation fails rather than falling back to the AI when its plan is unusable', async t => {
  const valid = plan('merge', base);
  for (const [name, answer] of [
    ['portal error', { status: 500 }],
    ['unauthorized key', { status: 401 }],
    ['unknown mode', { status: 200, plan: { ...valid, mode: 'rewrite' } }],
    ['unsafe version', { status: 200, plan: { ...valid, release: { ...valid.release, version: '<b>1</b>' } } }],
    ['forbidden path', { status: 200, plan: { ...valid, release: { ...valid.release, files: tree({ '.git/config': 'x' }) } } }],
  ]) {
    const { config } = await portal(t, () => answer);
    await assert.rejects(fetchPlan(config, { appId: 'btp', conversationId: randomUUID() }), name);
  }
});

test('a clean release merge keeps company edits, applies release edits and deletions, without the model', async t => {
  const ours = { ...base, 'src/theme.ts': 'export const color = "orange";\n' };
  const theirs = { 'src/sidebar.ts': sidebar('Chantiers & devis'), 'src/theme.ts': base['src/theme.ts'], 'src/new.ts': 'export const fresh = 1;\n' };
  const compiled = [];
  const { run, root, seen } = await update(t, { mode: 'merge', ours, theirs, compile: async dir => { compiled.push(decoded(await collect(dir))); return 'artifact'; }, coding: () => assert.fail('no model') });
  const result = await run;
  const expected = { 'src/sidebar.ts': sidebar('Chantiers & devis'), 'src/theme.ts': ours['src/theme.ts'], 'src/new.ts': theirs['src/new.ts'] };
  assert.deepEqual(decoded(result.files), expected);
  assert.deepEqual(compiled, [expected], 'the merged tree is what gets verified');
  assert.deepEqual(decoded(await collect(root)), expected);
  assert.equal(result.frontend, 'artifact');
  assert.match(result.summary, /0\.4\.1.*conservées/);
  assert.deepEqual(seen.reports, []);
});

test('binary assets merge byte for byte', async t => {
  const logo = bytes => ({ ...tree(base), 'public/logo.png': Buffer.from(bytes).toString('base64') });
  const original = logo([0x89, 0x0d, 0x0a, 0x00, 0xff]);
  const released = logo([0x89, 0x0d, 0x0a, 0x00, 0xfe, 0x0a]);
  const root = await mkdtemp(join(tmpdir(), 'builder-update-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  await materialize(root, original);
  const result = await applyUpdate({ plan: { mode: 'merge', base: { version: '0.3.0', files: original }, release: { version: '0.4.1', files: released } }, root, appId: 'btp', prompt: 'x', signal: new AbortController().signal, config: {}, compile: async () => 'artifact', report: () => assert.fail('clean merge') });
  assert.equal(result.files['public/logo.png'], released['public/logo.png']);
});

test('a conflicting release publishes nothing and reports the conflicted files', async t => {
  const ours = { ...base, 'src/sidebar.ts': sidebar('Mes chantiers') };
  const theirs = { ...base, 'src/sidebar.ts': sidebar('Chantiers & devis') };
  const { run, seen, conversationId } = await update(t, { mode: 'merge', ours, theirs, compile: () => assert.fail('no build'), coding: () => assert.fail('no model') });
  const result = await run;
  assert.deepEqual(result.files, tree(ours), 'returning the input files makes Core record an answer, not a revision');
  assert.equal(result.frontend, '');
  assert.deepEqual(seen.reports, [{ key: 'rk_tenant', appId: 'btp', conversationId, outcome: 'conflict', conflicts: ['src/sidebar.ts'] }]);
  assert.match(result.summary, /0\.4\.1/);
  assert.match(result.summary, /Mises à jour/);
  assert.doesNotMatch(result.summary, /src\/|sidebar|\.ts/);
});

test('reconciliation hands the conflict markers to the coder and refuses unresolved markers', async t => {
  const ours = { ...base, 'src/sidebar.ts': sidebar('Mes chantiers') };
  const theirs = { ...base, 'src/sidebar.ts': sidebar('Chantiers & devis') };
  for (const [name, resolve, outcome] of [['resolved', true, null], ['markers left', false, 'failed']]) {
    let received;
    const { run, seen } = await update(t, { mode: 'reconcile', ours, theirs, coding: async request => {
      received = { ...request, files: decoded(await collect(request.root)) };
      const files = await collect(request.root);
      if (resolve) files['src/sidebar.ts'] = encoded(sidebar('Mes chantiers & devis'));
      return { files, frontend: 'artifact', summary: 'Conciliée' };
    } });
    if (outcome) await assert.rejects(run, error => error.code === 'BUILD_FAILED', name);
    else assert.equal((await run).summary, 'Conciliée', name);
    assert.match(received.files['src/sidebar.ts'], /^<{7} [\s\S]*Mes chantiers[\s\S]*^={7}$[\s\S]*Chantiers & devis[\s\S]*^>{7} /m, name);
    assert.match(received.prompt, /0\.3\.0[\s\S]*0\.4\.1[\s\S]*src\/sidebar\.ts[\s\S]*Owner request: Mettre à jour$/, name);
    assert.equal(received.narrationPrompt, 'Mettre à jour', `${name}: narration stays on the owner's words`);
    assert.deepEqual(seen.reports.map(report => report.outcome), outcome ? [outcome] : [], name);
  }
});

test('a clean merge that fails verification reports failure and is not published', async t => {
  const theirs = { ...base, 'src/theme.ts': 'export const color = ;\n' };
  const { run, seen } = await update(t, { mode: 'merge', ours: base, theirs, compile: async () => { throw new Error('tsc: expression expected'); }, coding: () => assert.fail('no model') });
  await assert.rejects(run, error => error.code === 'BUILD_FAILED');
  assert.deepEqual(seen.reports.map(report => report.outcome), ['failed']);
});

test('conflicts without markers are named by kind and leftover Git copies are refused', async t => {
  const cases = [
    // A coder that keeps the company file publishes; one that leaves Git's side copy must not.
    ['deleted by the release, edited by the company', { ...base, 'src/legacy.ts': 'export const old = 2;\n' }, (({ 'src/legacy.ts': _, ...rest }) => rest)(base), 'src/legacy.ts', /deleted by the new version, edited by the company/, null],
    ['file against folder', { ...base, 'src/extra': 'company file\n' }, { ...base, 'src/extra/index.ts': 'export {};\n' }, 'src/extra', /file and a folder clash/, 'failed'],
  ];
  for (const [name, ours, theirs, path, kind, outcome] of cases) {
    let prompt;
    const { run, seen } = await update(t, { mode: 'reconcile', ours, theirs, coding: async request => {
      prompt = request.prompt;
      return { files: await collect(request.root), frontend: 'artifact', summary: 'Conciliée' };
    } });
    const result = await run.catch(error => error);
    assert.match(prompt, kind, name);
    assert.ok(prompt.includes(path), name);
    assert.equal(result.code, outcome ? 'BUILD_FAILED' : undefined, name);
    assert.deepEqual(seen.reports.map(report => report.outcome), outcome ? [outcome] : [], name);
  }
  const merge = await update(t, { mode: 'merge', ours: cases[1][1], theirs: cases[1][2], compile: () => assert.fail('no build'), coding: () => assert.fail('no model') });
  await merge.run;
  assert.ok(merge.seen.reports[0].conflicts.every(path => !/~(HEAD|release)$/.test(path)), 'reports name real paths, not Git side copies');
});
