import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { createInterface } from 'node:readline';
import test from 'node:test';

test('production entrypoint accepts in-memory configuration and enforces transport policy', async t => {
  const config = { token: 'a'.repeat(32), apiKey: 'provider-private-value', model: 'test-model', endpoint: 'http://host.docker.internal:4178/api/llm/v1/messages' };
  for (const local of [false, true]) {
    const child = spawn(process.execPath, ['server.mjs', '--config-stdin'], {
      cwd: new URL('..', import.meta.url),
      env: { ...process.env, PORT: '0', ROOTCX_BUILDER_ALLOW_LOCAL_HTTP: String(local) },
      stdio: ['pipe', 'pipe', 'pipe'],
    });
    t.after(() => child.kill('SIGKILL'));
    const exited = once(child, 'exit');
    let errors = '';
    child.stderr.on('data', bytes => { errors += bytes; });
    const lines = createInterface({ input: child.stdout });
    child.stdin.end(JSON.stringify(config) + '\n');
    if (!local) {
      assert.notEqual((await exited)[0], 0);
      assert.match(errors, /must use HTTPS/);
      assert.ok(!errors.includes(config.apiKey));
      continue;
    }
    let ready;
    for await (const line of lines) {
      const event = JSON.parse(line);
      if (event.event === 'builder.ready') { ready = event; break; }
    }
    assert.ok(ready, errors);
    const url = `http://127.0.0.1:${ready.port}`;
    assert.equal((await fetch(`${url}/health`)).status, 200);
    assert.equal((await fetch(`${url}/build`, { method: 'POST', body: '{}' })).status, 401);
    child.kill('SIGTERM');
    assert.equal((await exited)[0], 0);
  }
});
