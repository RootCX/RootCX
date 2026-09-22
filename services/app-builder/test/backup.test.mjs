import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm, readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { backupReceiver } from '../backup-receiver.mjs';

test('backup acknowledges persisted bytes and cannot overwrite an existing revision', async t => {
  const directory = await mkdtemp(join(tmpdir(), 'backup-test-'));
  const token = 'backup-test-token-000000000000000000';
  const server = backupReceiver({ token, directory });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  t.after(async () => { server.closeAllConnections(); await new Promise(resolve => server.close(resolve)); await rm(directory, { recursive: true, force: true }); });
  const path = `example/${'a'.repeat(40)}.bundle`;
  const url = `http://127.0.0.1:${server.address().port}/${path}`;
  const body = '# v2 git bundle\nfixture';
  assert.equal((await fetch(url, { method: 'PUT', body })).status, 401);
  const put = value => fetch(url, { method: 'PUT', headers: { authorization: `Bearer ${token}` }, body: value });
  assert.equal((await put(body)).status, 201);
  assert.equal(await readFile(join(directory, path), 'utf8'), body);
  assert.equal((await put(body)).status, 201, 'uncertain transport retry is idempotent');
  assert.equal((await put(body + 'different')).status, 409);
  assert.equal(await readFile(join(directory, path), 'utf8'), body);
});
