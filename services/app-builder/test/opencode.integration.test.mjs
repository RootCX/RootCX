import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { setTimeout as delay } from 'node:timers/promises';
import { code } from '../agent.mjs';
import { closeEngine, engine } from '../opencode.mjs';

test('real OpenCode reports an offline gateway without pretending to wait', { skip: process.platform !== 'linux', timeout: 30000 }, async t => {
  const root = await mkdtemp('/tmp/opencode-offline-');
  const reservation = createServer();
  await new Promise(resolve => reservation.listen(0, '127.0.0.1', resolve));
  const endpoint = `http://127.0.0.1:${reservation.address().port}/v1/messages`;
  await new Promise(resolve => reservation.close(resolve));
  t.after(async () => { await closeEngine(root); await rm(root, { recursive: true, force: true }); await rm(`${root}.agent`, { recursive: true, force: true }); });
  const stages = [];
  await assert.rejects(code({ root, appId: 'offline', prompt: 'Hello', signal: AbortSignal.timeout(20000), config: { model: 'claude-opus-4-6', endpoint, apiKey: 'test' }, onProgress: phase => stages.push(phase) }), { code: 'AI_CONFIGURATION' });
  assert.ok(!stages.includes('waiting'), JSON.stringify(stages));
});

test('real OpenCode loads RootCX skill, edits, cancels running tools and resumes the conversation', { skip: process.platform !== 'linux', timeout: 120000 }, async t => {
  const root = await mkdtemp('/tmp/opencode-test-');
  await mkdir(`${root}/src`); await writeFile(`${root}/src/app.ts`, 'original');
  let turn = 0; let skillResult = false; let skillError; const stages = [];
  let command = "printf 'changed' > src/app.ts";
  const provider = createServer(async (req, res) => {
    const chunks = []; for await (const chunk of req) chunks.push(chunk);
    const input = JSON.parse(Buffer.concat(chunks));
    const primary = input.tools?.some(tool => tool.name === 'skill');
    const responses = [
      { type: 'tool_use', id: 'tool_skill', name: 'skill', input: { id: 'rootcx' } },
      { type: 'tool_use', id: 'tool_edit', name: 'shell', input: { command, description: 'Adapt the application' } },
      { type: 'text', text: 'La modification est préparée.' },
    ];
    if (primary && turn > 0) {
      skillResult ||= JSON.stringify(input.messages).includes('Manifest defines the app contract');
      for (const message of input.messages) {
        if (!Array.isArray(message.content)) continue;
        const result = message.content.find(part => part.type === 'tool_result' && part.tool_use_id === 'tool_skill' && part.is_error);
        if (result) skillError = JSON.stringify(result.content).slice(0, 2000);
      }
    }
    const block = primary ? responses[Math.min(turn++, 2)] : { type: 'text', text: 'Modification application' };
    const reason = block.type === 'tool_use' ? 'tool_use' : 'end_turn';
    const message = { id: `msg_${Date.now()}`, type: 'message', role: 'assistant', model: input.model, content: [block], stop_reason: reason, stop_sequence: null, usage: { input_tokens: 10, output_tokens: 10 } };
    if (!input.stream) { res.writeHead(200, { 'content-type': 'application/json' }); res.end(JSON.stringify(message)); return; }
    res.writeHead(200, { 'content-type': 'text/event-stream' });
    const emit = (type, data) => res.write(`event: ${type}\ndata: ${JSON.stringify({ type, ...data })}\n\n`);
    emit('message_start', { message: { ...message, content: [], stop_reason: null } });
    emit('content_block_start', { index: 0, content_block: block.type === 'tool_use' ? { ...block, input: {} } : { type: 'text', text: '' } });
    emit('content_block_delta', { index: 0, delta: block.type === 'tool_use' ? { type: 'input_json_delta', partial_json: JSON.stringify(block.input) } : { type: 'text_delta', text: block.text } });
    emit('content_block_stop', { index: 0 });
    emit('message_delta', { delta: { stop_reason: reason, stop_sequence: null }, usage: { output_tokens: 10 } });
    emit('message_stop', {}); res.end();
  });
  await new Promise(resolve => provider.listen(0, '127.0.0.1', resolve));
  t.after(async () => { await closeEngine(root); provider.closeAllConnections(); provider.close(); await rm(root, { recursive: true, force: true }); await rm(`${root}.agent`, { recursive: true, force: true }); });
  const config = { model: 'claude-opus-4-6', endpoint: `http://127.0.0.1:${provider.address().port}/v1/messages`, apiKey: 'test' };
  let publications = 0;
  const compile = async () => { publications++; return Buffer.from('artifact').toString('base64'); };
  const result = await code({ root, appId: 'sample', prompt: 'Change the application', signal: AbortSignal.timeout(90000), config, onProgress: phase => stages.push(phase), compile });
  assert.equal(await readFile(`${root}/src/app.ts`, 'utf8'), 'changed');
  assert.equal(result.files['src/app.ts'], Buffer.from('changed').toString('base64'));
  assert.ok(skillResult, `the native skill content must reach the model: ${skillError ?? 'no tool result'}`);
  assert.ok(stages.includes('checking'), JSON.stringify(stages));
  const runtime = await engine(root, config);
  const session = await runtime.conversation();
  command = "printf 'started' > CANCEL-STARTED; sleep 2; printf 'late' > CANCEL-LATE";
  turn = 1;
  const cancellation = new AbortController();
  const running = code({ root, appId: 'sample', prompt: 'Start the cancellable operation', signal: cancellation.signal, config, compile });
  const interrupted = assert.rejects(running, { name: 'AbortError' });
  const deadline = Date.now() + 5000;
  while (true) {
    try { assert.equal(await readFile(`${root}/CANCEL-STARTED`, 'utf8'), 'started'); break; }
    catch (error) { if (error.code !== 'ENOENT' || Date.now() >= deadline) throw error; }
    await delay(20);
  }
  cancellation.abort();
  await interrupted;
  await delay(2200);
  await assert.rejects(readFile(`${root}/CANCEL-LATE`), { code: 'ENOENT' });
  assert.equal(publications, 1, 'an interrupted operation must never publish');
  assert.equal(await runtime.conversation(), session);
  turn = 2;
  const resumed = await code({ root, appId: 'sample', prompt: 'Resume our conversation', signal: AbortSignal.timeout(10000), config, compile });
  assert.equal(resumed.summary, 'La modification est préparée.');
});
