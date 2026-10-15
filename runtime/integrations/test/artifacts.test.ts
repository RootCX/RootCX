import { expect, test } from 'bun:test';
import { readFile, mkdtemp, copyFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { resolve } from 'node:path';
import { plugins } from '../src/registry';

test('compiled workers dispatch real actions through the same protocol without runtime dependencies', async () => {
  const previousServe = (globalThis as any).serve;
  const previousFetch = globalThis.fetch;
  try {
    for (const plugin of plugins) {
      let rpc: any;
      (globalThis as any).serve = (handlers: any) => { rpc = handlers.rpc; };
      let calls = 0;
      globalThis.fetch = (async () => { calls++; return new Response('', { status: 401 }); }) as unknown as typeof fetch;
      const legacy = ['gmail', 'google_calendar'].includes(plugin.id);
      const folder = resolve(import.meta.dir, '../../../core/resources/integrations', plugin.id);
      const imported = await import(resolve(folder, legacy ? 'plugin.ts' : 'index.ts'));
      if (legacy) rpc = imported.rpc;
      const definition = JSON.parse(await readFile(resolve(folder, 'manifest.json'), 'utf8'));
      const action = legacy ? 'live_verify_connection' : 'verify_connection';
      expect(definition.actions.some((entry: any) => entry.id === action)).toBe(true);
      expect(await rpc.__integration({ action, input: {}, connectionId: crypto.randomUUID(), userCredentials: { accessToken: 'fixture', instanceUrl: 'https://fixture.my.salesforce.com' } })).toMatchObject({ ok: false, error: { code: 'unauthorized' } });
      expect(calls).toBe(1);
    }
  } finally { (globalThis as any).serve = previousServe; globalThis.fetch = previousFetch; }
});


test('legacy integration packages install with their shipped frozen lockfile', async () => {
  for (const id of ['gmail', 'google_calendar']) {
    const folder = resolve(import.meta.dir, '../../../core/resources/integrations', id);
    const temporary = await mkdtemp(resolve(tmpdir(), `rootcx-${id}-lock-`));
    try {
      for (const file of ['package.json', 'bun.lock']) await copyFile(resolve(folder, file), resolve(temporary, file));
      // Exercise Bun's actual resolver without installing packages or running lifecycle scripts.
      const child = Bun.spawn([process.execPath, 'install', '--frozen-lockfile', '--lockfile-only', '--ignore-scripts'], {
        cwd: temporary, stdout: 'pipe', stderr: 'pipe',
      });
      const [exit, stdout, stderr] = await Promise.all([child.exited, new Response(child.stdout).text(), new Response(child.stderr).text()]);
      expect({ id, exit, output: exit === 0 ? '' : stdout + stderr }).toEqual({ id, exit: 0, output: '' });
    } finally { await rm(temporary, { recursive: true, force: true }); }
  }
}, 30_000);
