import { setTimeout as delay } from 'node:timers/promises';
import { readFile } from 'node:fs/promises';
import { collect, confined, edit, remove, validateFiles } from './workspace.mjs';
import { build } from './sandbox.mjs';

const string = { type: 'string' };
const tools = [
  ['list_files', 'List application source files. Dependencies and build outputs are excluded.', {}, []],
  ['read_file', 'Read an application source file as text. Read existing files before editing them.', { path: string }, ['path']],
  ['write_file', 'Create or replace an application source file with complete UTF-8 contents. Keep changes focused on the request.', { path: string, content: string }, ['path', 'content']],
  ['replace_text', 'Replace exactly one occurrence in an existing source file. Fails if the text is missing or ambiguous.', { path: string, before: string, after: string }, ['path', 'before', 'after']],
  ['delete_file', 'Delete one source file. Does not allow removing directories or dependencies.', { path: string }, ['path']],
  ['check', 'Typecheck and build the real application in an isolated environment. Fix reported failures before finishing.', {}, []],
].map(([name, description, properties, required]) => ({ name, description, input_schema: { type: 'object', properties, required, additionalProperties: false } }));

export async function code({ root, appId, prompt, signal, config, request = fetch, compile = build }) {
  const instructions = await readFile(new URL('./instructions.md', import.meta.url), 'utf8');
  const messages = [{ role: 'user', content: `Application: ${appId}\nRequest: ${prompt}` }];
  let tokens = 0;
  for (let turn = 0; turn < 40; turn++) {
    let response;
    for (let attempt = 0; attempt < 3; attempt++) {
      response = await request(config.endpoint, {
      method: 'POST', redirect: 'error', signal: AbortSignal.any([...(signal ? [signal] : []), AbortSignal.timeout(90_000)]),
      headers: { 'content-type': 'application/json', 'x-api-key': config.apiKey, 'anthropic-version': '2023-06-01' },
      body: JSON.stringify({ model: config.model, max_tokens: 8192, system: instructions, tools, messages }),
    });
      if (![429, 502, 503, 504, 529].includes(response.status) || attempt === 2) break;
      await response.body?.cancel();
      await delay(500 * 2 ** attempt, undefined, { signal });
    }
    if (!response.ok) {
      const error = new Error(`Coding provider returned ${response.status}`);
      if (response.status === 402) error.code = 'AI_CREDITS_EXHAUSTED';
      throw error;
    }
    const result = await response.json();
    tokens += (result.usage?.input_tokens ?? 0) + (result.usage?.output_tokens ?? 0);
    if (tokens > 500_000) throw new Error('Coding budget exhausted');
    if (!Array.isArray(result.content) || result.stop_reason === 'max_tokens') throw new Error('Incomplete model response');
    messages.push({ role: 'assistant', content: result.content });
    const calls = result.content.filter(block => block.type === 'tool_use');
    if (!calls.length) {
      // Completion always rebuilds the final files; an earlier successful check
      // must not authorize files edited afterwards.
      const before = await collect(root); validateFiles(before);
      const frontend = await compile(root, appId, signal);
      const files = await collect(root); validateFiles(files);
      if (JSON.stringify(before) !== JSON.stringify(files)) throw new Error('Build modified source files; refusing an unverified source revision');
      return { files, frontend, summary: result.content.filter(block => block.type === 'text').map(block => block.text).join('\n').slice(0, 2000) };
    }
    const results = [];
    for (const call of calls) {
      try {
        const input = call.input ?? {};
        let content;
        switch (call.name) {
          case 'list_files': content = Object.keys(await collect(root)).join('\n'); break;
          case 'read_file': {
            const bytes = await readFile(await confined(root, input.path));
            if (bytes.length > 256_000) throw new Error('File exceeds read limit');
            content = bytes.toString('utf8'); break;
          }
          case 'write_file': await edit(root, input.path, input.content); content = 'Saved'; break;
          case 'replace_text': {
            const text = await readFile(await confined(root, input.path), 'utf8');
            if (typeof input.before !== 'string' || !input.before || typeof input.after !== 'string' || text.split(input.before).length !== 2) throw new Error('Expected one exact match');
            await edit(root, input.path, text.replace(input.before, () => input.after)); content = 'Saved'; break;
          }
          case 'delete_file': await remove(root, input.path); content = 'Removed'; break;
          case 'check': await compile(root, appId, signal); content = 'Typecheck and frontend build passed'; break;
          default: throw new Error('Unknown tool');
        }
        results.push({ type: 'tool_result', tool_use_id: call.id, content });
      } catch (error) { results.push({ type: 'tool_result', tool_use_id: call.id, content: String(error.message).slice(-24000), is_error: true }); }
    }
    messages.push({ role: 'user', content: results });
    if (JSON.stringify(messages).length > 2_000_000) throw new Error('Conversation context limit exceeded');
  }
  throw new Error('Coding turn limit exceeded');
}
