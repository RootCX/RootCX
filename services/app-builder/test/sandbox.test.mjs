import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, writeFile, rm, cp, mkdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { isolated, build } from '../sandbox.mjs';

test('build sandbox cannot read runner secrets, sibling applications or the network', { skip: process.platform !== 'linux' }, async t => {
  const root = await mkdtemp(join(tmpdir(), 'sandbox-test-'));
  const outside = await mkdtemp(join(tmpdir(), 'other-application-'));
  t.after(async () => { await rm(root, { recursive: true, force: true }); await rm(outside, { recursive: true, force: true }); });
  await writeFile(join(outside, 'secret'), 'never expose');
  process.env.ROOTCX_TEST_SECRET = 'never expose';
  const result = await isolated(root, ['node', '-e', `
    const fs=require('node:fs');
    if(process.env.ROOTCX_TEST_SECRET)process.exit(2);
    if(fs.existsSync(${JSON.stringify(join(outside, 'secret'))}))process.exit(3);
    if(fs.existsSync('/data/RootCX'))process.exit(4);
    const pids=fs.readdirSync('/proc').filter(p=>/^\\d+$/.test(p));
    for(const pid of pids){try{if(fs.readFileSync('/proc/'+pid+'/environ').includes('ROOTCX_TEST_SECRET'))process.exit(5)}catch{}}
    fetch('https://example.com',{signal:AbortSignal.timeout(1500)}).then(()=>process.exit(6),()=>console.log('isolated'));
  `]);
  assert.match(result, /isolated/);
});

test('publication checks report only operations actually run and never turn failure into success', { skip: process.platform !== 'linux' || process.env.ROOTCX_SANDBOX_NETWORK_TESTS !== 'true', timeout: 120000 }, async t => {
  const root = await mkdtemp(join(tmpdir(), 'nova-checks-'));
  t.after(async () => { for (const path of [root, `${root}.sandbox`]) await rm(path, { recursive: true, force: true }); });
  await cp(new URL('./fixture/', import.meta.url), root, { recursive: true });
  await mkdir(join(root, 'backend'), { recursive: true });
  const file = join(root, 'backend', 'rule.test.ts');
  await writeFile(file, "import { test, expect } from 'bun:test'; test('acompte', () => expect(300).toBe(400));");
  const facts = [];
  await assert.rejects(build(root, 'fixture', AbortSignal.timeout(90000), fact => facts.push(fact)));
  assert.deepEqual(facts.filter(fact => fact.action === 'test').map(fact => fact.status), ['running', 'error']);
  assert.ok(!facts.some(fact => fact.action === 'build'), 'a failed test stops version preparation');
  await writeFile(file, "import { test, expect } from 'bun:test'; test('acompte', () => expect(400).toBe(400));");
  facts.length = 0;
  assert.ok(await build(root, 'fixture', AbortSignal.timeout(90000), fact => facts.push(fact)));
  assert.deepEqual(facts.filter(fact => fact.action === 'test').map(fact => fact.status), ['running', 'completed']);
  assert.deepEqual(facts.filter(fact => fact.action === 'build').map(fact => fact.status), ['running', 'completed']);
});
