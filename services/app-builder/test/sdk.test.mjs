import test from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createServer } from 'node:http';
import { createInterface } from 'node:readline';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { eventAdapter } from '../sdk-host.mjs';
import { progressFor, providerError } from '../opencode.mjs';

test('SDK activity preserves checking and repair without forwarding tool output or reasoning', () => {
  const translate = eventAdapter();
  const events = [
    { type: 'session.tool.input.started', data: { sessionID: 'one', id: 'tool', name: 'shell' } },
    { type: 'session.tool.called', data: { sessionID: 'one', id: 'tool', input: { command: 'npm test' } } },
    { type: 'session.tool.failed', data: { sessionID: 'one', id: 'tool', error: { message: 'private output' } } },
    { type: 'session.reasoning.ended', data: { sessionID: 'one', text: 'private reasoning' } },
  ].map(translate).filter(Boolean);
  assert.deepEqual(events.map(progressFor), ['checking', 'repairing']);
  assert.doesNotMatch(JSON.stringify(events), /private/);
  assert.equal(providerError({ type: 'provider.error', status: 402 }), 'AI_CREDITS_EXHAUSTED');
});

test('published SDK executes through pipes, ignores project plugins, preserves sessions and reports provider errors', { timeout: 60000 }, async t => {
  const directory = await mkdtemp(join(tmpdir(), 'rootcx-sdk-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const root = join(directory, 'app'), home = join(directory, 'state');
  await Promise.all([mkdir(root), mkdir(home)]);
  await writeFile(join(root, 'opencode.json'), JSON.stringify({ plugins: ['./malicious.mjs'], model: 'missing/invalid' }));
  await writeFile(join(root, 'malicious.mjs'), `throw new Error('Project plugin must never execute');`);
  let status = 200;
  let tool;
  let stalled;
  let stalledRequest;
  const requests = [];
  const server = createServer(async (req, res) => {
    let body = ''; for await (const chunk of req) body += chunk;
    requests.push({ path: req.url, key: req.headers['x-api-key'], body: JSON.parse(body) });
    if (stalled) { stalledRequest(); return; }
    if (status !== 200) { res.writeHead(status, { 'content-type': 'application/json' }); res.end(JSON.stringify({ type: 'error', error: { type: 'invalid_request_error', message: 'No credits' } })); return; }
    const answer = 'Votre calendrier est prêt.';
    if (!JSON.parse(body).stream) {
      res.writeHead(200, { 'content-type': 'application/json' }); res.end(JSON.stringify({ id: 'msg_test', type: 'message', role: 'assistant', model: 'claude-opus-4-6', content: [{ type: 'text', text: answer }], stop_reason: 'end_turn', usage: { input_tokens: 10, output_tokens: 5 } })); return;
    }
    res.writeHead(200, { 'content-type': 'text/event-stream' });
    const command = tool; tool = undefined;
    for (const data of [
      { type: 'message_start', message: { id: 'msg_test', type: 'message', role: 'assistant', model: 'claude-opus-4-6', content: [], stop_reason: null, usage: { input_tokens: 10, output_tokens: 0 } } },
      { type: 'content_block_start', index: 0, content_block: command ? { type: 'tool_use', id: 'tool_test', name: 'shell', input: {} } : { type: 'text', text: '' } },
      { type: 'content_block_delta', index: 0, delta: command ? { type: 'input_json_delta', partial_json: JSON.stringify({ command }) } : { type: 'text_delta', text: answer } },
      { type: 'content_block_stop', index: 0 },
      { type: 'message_delta', delta: { stop_reason: command ? 'tool_use' : 'end_turn', stop_sequence: null }, usage: { output_tokens: 5 } },
      { type: 'message_stop' },
    ]) res.write(`event: ${data.type}\ndata: ${JSON.stringify(data)}\n\n`);
    res.end();
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  t.after(() => { server.closeAllConnections(); server.close(); });
  const config = { root, home, providerUrl: `http://127.0.0.1:${server.address().port}`, proxyKey: 'scoped-test-key', model: 'claude-opus-4-6', instructions: 'Respond briefly.', skills: join(directory, 'skills'), configDirectory: join(directory, 'config') };
  const events = [];
  async function launch() {
    const child = spawn('bun', [fileURLToPath(new URL('../sdk-host.mjs', import.meta.url))], { cwd: root, env: { PATH: process.env.PATH, HOME: home, XDG_DATA_HOME: join(home, 'data'), XDG_CONFIG_HOME: join(home, 'config'), XDG_CACHE_HOME: join(home, 'cache'), XDG_STATE_HOME: join(home, 'state') }, stdio: ['pipe', 'pipe', 'pipe'] });
    t.after(() => child.kill('SIGKILL'));
    let diagnostics = ''; child.stderr.on('data', chunk => { diagnostics += chunk; });
    const pending = new Map(); let id = 0, readyResolve, readyReject;
    const ready = new Promise((resolve, reject) => { readyResolve = resolve; readyReject = reject; });
    child.on('error', readyReject);
    child.on('exit', code => { const error = new Error(`SDK exited (${code}): ${diagnostics}`); readyReject(error); for (const value of pending.values()) value.reject(error); });
    createInterface({ input: child.stdout }).on('line', line => {
      const message = JSON.parse(line);
      if (message.type === 'ready') readyResolve();
      else if (message.type === 'event') events.push(message.event);
      else if (message.type === 'fatal') readyReject(new Error(JSON.stringify(message.error)));
      else if (message.type === 'response') {
        const value = pending.get(message.id); pending.delete(message.id);
        if (message.error) value.reject(new Error(JSON.stringify(message.error))); else value.resolve(message.result);
      }
    });
    child.stdin.write(JSON.stringify(config) + '\n'); await ready;
    return {
      api(path, body) { return new Promise((resolve, reject) => { const next = ++id; pending.set(next, { resolve, reject }); child.stdin.write(JSON.stringify({ id: next, path, body }) + '\n'); }); },
      close() { return new Promise(resolve => { child.once('exit', resolve); child.stdin.end(); }); },
    };
  }
  let host = await launch();
  const session = await host.api('/session');
  const first = await host.api(`/session/${session.id}/message`, { parts: [{ type: 'text', text: 'Bonjour' }] });
  assert.equal(first.info.finish, 'stop');
  assert.equal(first.parts[0].text, 'Votre calendrier est prêt.');
  assert.ok(events.some(event => event.properties?.part?.type === 'text'));
  assert.deepEqual([...new Set(requests.map(request => request.path))], ['/v1/messages?beta=true']);
  assert.ok(requests.every(request => request.key === 'scoped-test-key'));
  tool = "printf 'sdk-tool-ok' > SDK-PROOF.txt";
  await host.api(`/session/${session.id}/message`, { parts: [{ type: 'text', text: 'Créer la preuve locale' }] });
  assert.equal(await readFile(join(root, 'SDK-PROOF.txt'), 'utf8'), 'sdk-tool-ok');
  assert.ok(events.some(event => event.properties?.part?.tool === 'shell' && event.properties.part.state.status === 'running'));
  await host.close();
  host = await launch();
  assert.equal((await host.api(`/session/${session.id}`)).id, session.id);
  status = 402;
  const failure = await host.api(`/session/${session.id}/message`, { parts: [{ type: 'text', text: 'Encore' }] });
  assert.equal(providerError(failure.info.error), 'AI_CREDITS_EXHAUSTED');
  status = 200;
  stalled = true;
  const requested = new Promise(resolve => { stalledRequest = resolve; });
  const running = host.api(`/session/${session.id}/message`, { parts: [{ type: 'text', text: 'Interrompre cette requête' }] });
  running.catch(() => {});
  await requested;
  assert.equal(await host.api(`/session/${session.id}/abort`), true);
  const interrupted = await running;
  assert.ok(interrupted.info.error || interrupted.info.finish !== 'stop');
  await host.close();
});
