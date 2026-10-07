import { createInterface } from 'node:readline';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

export function sdkSettings({ root, model, providerUrl, proxyKey, instructions, skills = '/opt/rootcx-skills' }) {
  return {
    model: `anthropic/${model}`, share: 'disabled', update: 'disable', snapshots: false,
    enabled_providers: ['anthropic'], skills: [skills],
    permissions: [{ action: '*', resource: '*', effect: 'allow' }, { action: 'question', resource: '*', effect: 'deny' }],
    providers: { anthropic: { settings: { baseURL: `${providerUrl}/v1`, apiKey: proxyKey }, models: { [model]: { name: model, limit: { context: 200000, output: 16000 } } } } },
    agents: {
      build: { system: instructions.replaceAll('/workspace', root) + '\nThe RootCX documentation is packaged in /opt/rootcx-docs. Read those local files instead of fetching rootcx.com. The environment intentionally has no general Internet access.', steps: 80 },
      title: { model: `anthropic/${model}` },
    },
  };
}

// Keep the public activity vocabulary independent of SDK event versions.
export function eventAdapter() {
  const tools = new Map();
  return event => {
    const data = event.data;
    if (!data?.sessionID) return null;
    const properties = { sessionID: data.sessionID };
    if (event.type === 'session.status') {
      if (data.status.type === 'idle') for (const key of tools.keys()) if (key.startsWith(`${data.sessionID}:`)) tools.delete(key);
      return { type: 'session.status', properties: { ...properties, status: data.status } };
    }
    if (event.type === 'session.text.ended') return { type: 'message.part.updated', properties: { ...properties, part: { type: 'text', sessionID: data.sessionID, text: data.text, time: { end: event.created } } } };
    if (event.type === 'session.skill.activated') return { type: 'message.part.updated', properties: { ...properties, part: { type: 'tool', sessionID: data.sessionID, tool: 'skill', state: { status: 'completed', input: { name: data.name } } } } };
    const key = `${data.sessionID}:${data.id}`;
    if (event.type === 'session.tool.input.started') { tools.set(key, { tool: data.name, input: {} }); return null; }
    const tool = tools.get(key);
    if (!tool) return null;
    let status;
    if (event.type === 'session.tool.called') { tool.input = data.input; status = 'running'; }
    else if (event.type === 'session.tool.success') {
      status = 'completed';
      if (tool.tool === 'skill') tool.input = { ...tool.input, name: data.metadata?.name ?? tool.input.id };
    }
    else if (event.type === 'session.tool.failed') status = 'error';
    else return null;
    if (status !== 'running') tools.delete(key);
    return { type: 'message.part.updated', properties: { ...properties, part: { type: 'tool', sessionID: data.sessionID, tool: tool.tool, state: { status, input: tool.input } } } };
  };
}

export async function createSdkHost(config, emit) {
  const { OpenCode } = await import('@opencode/sdk');
  const sdk = await OpenCode.create({
    app: { name: 'shappy', version: '2.0.24' },
    database: { path: join(config.home, 'opencode-v2.db') },
    config: { project: false, directory: config.configDirectory ?? fileURLToPath(new URL('./empty-config', import.meta.url)), content: JSON.stringify(sdkSettings(config)) },
    models: { fetch: false }, fs: { filewatcher: false, fff: false },
    log: { level: 'error', emit: entry => process.stderr.write(JSON.stringify(entry) + '\n') },
  });
  const adapter = eventAdapter();
  const executionErrors = new Map();
  const stream = new AbortController();
  let connected;
  const ready = new Promise(resolve => { connected = resolve; });
  let eventFailure;
  const observing = (async () => {
    for await (const event of sdk.events.subscribe({ signal: stream.signal })) {
      if (event.type === 'server.connected') connected();
      if (event.type === 'session.execution.failed') executionErrors.set(event.data.sessionID, event.data.error);
      const translated = adapter(event);
      if (translated) emit(translated);
    }
    if (!stream.signal.aborted) throw new Error('OpenCode event stream disconnected');
  })();
  observing.catch(error => { eventFailure = error; connected(); });
  await Promise.race([ready, new Promise((_, reject) => { const timer = setTimeout(() => reject(new Error('OpenCode events unavailable')), 10000); timer.unref(); })]);
  if (eventFailure) { await sdk.close(); throw eventFailure; }
  return {
    async api(path, body) {
      if (eventFailure) throw eventFailure;
      if (path === '/session') return sdk.sessions.create({ location: { directory: config.root }, title: 'Shappy', agent: 'build', model: { providerID: 'anthropic', id: config.model } });
      const match = /^\/session\/([a-zA-Z0-9_-]+)(?:\/(message|abort))?$/.exec(path);
      if (!match) throw new Error('Unsupported coding operation');
      const sessionID = match[1];
      if (!match[2]) return sdk.sessions.get({ sessionID });
      if (match[2] === 'abort') { await sdk.sessions.interrupt({ sessionID, resume: false }); await sdk.sessions.wait({ sessionID }); return true; }
      const previous = new Set((await sdk.sessions.context({ sessionID })).map(message => message.id));
      executionErrors.delete(sessionID);
      await sdk.sessions.prompt({ sessionID, text: body.parts.map(part => part.text ?? '').join('\n') });
      await Promise.race([sdk.sessions.wait({ sessionID }), observing]);
      const state = await sdk.sessions.get({ sessionID });
      const messages = (await sdk.sessions.context({ sessionID })).filter(message => message.type === 'assistant' && !previous.has(message.id));
      const last = messages.at(-1);
      if (!last && state.outcome !== 'succeeded') throw new Error('OpenCode did not complete the request');
      return { info: { error: executionErrors.get(sessionID) ?? last?.error, finish: last?.finish }, parts: (last?.content ?? []).filter(part => part.type === 'text') };
    },
    async close() { stream.abort(); await observing.catch(() => {}); await sdk.close(); },
  };
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const output = value => process.stdout.write(JSON.stringify(value) + '\n');
  const lines = createInterface({ input: process.stdin });
  let host;
  let initializing;
  lines.on('line', line => {
    if (line.length > 1_000_000) { process.exitCode = 1; lines.close(); return; }
    let request;
    try { request = JSON.parse(line); } catch { process.exitCode = 1; lines.close(); return; }
    if (!initializing) {
      initializing = createSdkHost(request, event => output({ type: 'event', event })).then(value => { host = value; output({ type: 'ready' }); });
      initializing.catch(error => { output({ type: 'fatal', error: { message: error.message } }); process.exitCode = 1; lines.close(); });
      return;
    }
    void initializing.then(() => host.api(request.path, request.body)).then(result => output({ type: 'response', id: request.id, result }), error => output({ type: 'response', id: request.id, error: { message: error.message, status: error.status, data: error.data } }));
  });
  lines.on('close', () => { void initializing?.then(() => host?.close()).finally(() => process.exit(process.exitCode ?? 0)); });
}
