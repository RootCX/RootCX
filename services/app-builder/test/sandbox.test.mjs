import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { isolated } from '../sandbox.mjs';

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
