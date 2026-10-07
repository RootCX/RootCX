import { randomBytes } from 'node:crypto';
import { mkdir, readFile, rename, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createInterface } from 'node:readline';
import { createProviderProxy } from './provider-proxy.mjs';
import { spawnSandbox } from './sandbox.mjs';

const engines = new Map();

export function providerError(error) {
  let body;
  try { body = JSON.parse(error?.response?.body ?? error?.data?.responseBody); } catch {}
  if (body?.error?.code === 'AI_CONFIGURATION') return 'AI_CONFIGURATION';
  const status = error?.status ?? error?.data?.statusCode;
  if (status === 402) return 'AI_CREDITS_EXHAUSTED';
  if ([401, 403].includes(status) || ['ProviderAuthError', 'provider.auth'].includes(error?.type ?? error?.name)) return 'AI_CONFIGURATION';
  if (status === 429 || status >= 500) return 'AI_UNAVAILABLE';
  if (['ContextOverflowError', 'MessageOutputLengthError', 'provider.invalid-output'].includes(error?.type ?? error?.name)) return 'AI_LIMIT_REACHED';
  return 'BUILD_FAILED';
}

// Raw model text, reasoning, paths and command output never become public progress.
export function progressFor(event) {
  if (event.type === 'session.status' && event.properties.status?.type === 'retry') return 'waiting';
  if (event.type === 'session.status' && event.properties.status?.type === 'busy') return 'understanding';
  const part = event.properties?.part;
  if (part?.type !== 'tool') return null;
  if (part.state?.status === 'error') return 'repairing';
  if (['edit', 'write', 'apply_patch', 'patch'].includes(part.tool)) return 'editing';
  if (['bash', 'shell'].includes(part.tool)) {
    const command = String(part.state?.input?.command ?? '');
    if (/\b(?:tsc|vitest|jest|pytest|test|check|typecheck|build)\b/.test(command)) return 'checking';
    if (/^\s*(?:cat|ls|pwd|rg|grep|find|head|tail)\b/.test(command)) return 'understanding';
    return 'editing';
  }
  if (['skill', 'read', 'glob', 'grep', 'list', 'webfetch'].includes(part.tool)) return 'understanding';
  return null;
}

export async function engine(root, config) {
  if (engines.has(root)) return engines.get(root);
  const promise = startEngine(root, config);
  engines.set(root, promise);
  try {
    return await promise;
  } catch (error) {
    engines.delete(root);
    error.code = 'AI_CONFIGURATION';
    throw error;
  }
}

export async function closeEngine(root) {
  const pending = engines.get(root);
  engines.delete(root);
  if (!pending) return;
  const runtime = await pending.catch(() => null);
  await runtime?.close();
}

async function startEngine(root, config) {
  const home = `${root}.agent`;
  await mkdir(home, { recursive: true });
  const proxyKey = randomBytes(32).toString('hex');
  const proxy = createProviderProxy(config, proxyKey);
  await new Promise((resolve, reject) => { proxy.once('error', reject); proxy.listen(0, '127.0.0.1', resolve); });
  const providerUrl = `http://127.0.0.1:${proxy.address().port}`;
  let child;
  try {
    child = await spawnSandbox(root, ['bun', fileURLToPath(new URL('./sdk-host.mjs', import.meta.url))], { home, providerUrl });
  } catch (error) { proxy.close(); throw error; }
  const pending = new Map();
  const subscriptions = new Set();
  let sequence = 0;
  let failure;
  let diagnostics = '';
  let resolveReady, rejectReady;
  const ready = new Promise((resolve, reject) => { resolveReady = resolve; rejectReady = reject; });
  function failed(error) {
    if (failure) return;
    failure = error;
    engines.delete(root);
    rejectReady(error);
    for (const request of pending.values()) request.reject(error);
    pending.clear();
    for (const subscription of subscriptions) subscription.wake();
    proxy.closeAllConnections(); proxy.close();
  }
  child.stderr.on('data', chunk => { diagnostics = (diagnostics + chunk).slice(-8000); });
  child.on('error', failed);
  child.on('exit', code => failed(new Error(`OpenCode SDK stopped (${code}): ${diagnostics}`)));
  const lines = createInterface({ input: child.stdout });
  lines.on('line', line => {
    try {
      if (line.length > 2_000_000) throw new Error('OpenCode message exceeds limit');
      const message = JSON.parse(line);
      if (message.type === 'ready') resolveReady();
      else if (message.type === 'fatal') failed(new Error(message.error.message));
      else if (message.type === 'event') {
        for (const subscription of subscriptions) {
          if (subscription.queue.length > 1000) throw new Error('OpenCode event consumer stalled');
          subscription.queue.push(message.event); subscription.wake();
        }
      } else if (message.type === 'response') {
        const request = pending.get(message.id);
        if (!request) return;
        pending.delete(message.id);
        if (message.error) request.reject(Object.assign(new Error(message.error.message), message.error));
        else request.resolve(message.result);
      }
    } catch (error) { failed(error); void close(); }
  });
  let closing;
  function close() {
    return closing ??= new Promise(resolve => {
      failed(new Error('OpenCode SDK closed'));
      lines.close();
      if (child.exitCode !== null || child.signalCode !== null) return resolve();
      child.once('close', resolve);
      try { process.kill(-child.pid, 'SIGKILL'); } catch { child.kill('SIGKILL'); }
    });
  }
  function api(path, body, signal) {
    if (failure) return Promise.reject(failure);
    if (signal?.aborted) return Promise.reject(signal.reason);
    const id = ++sequence;
    return new Promise((resolve, reject) => {
      const abort = () => { pending.delete(id); reject(signal.reason); };
      signal?.addEventListener('abort', abort, { once: true });
      const finish = callback => value => { signal?.removeEventListener('abort', abort); callback(value); };
      pending.set(id, { resolve: finish(resolve), reject: finish(reject) });
      child.stdin.write(JSON.stringify({ id, path, body }) + '\n', error => { if (error) failed(error); });
    });
  }
  async function* subscribe(signal) {
    const subscription = { queue: [], wake() {} };
    subscriptions.add(subscription);
    const wake = () => subscription.wake();
    signal.addEventListener('abort', wake);
    try {
      while (!signal.aborted) {
        if (failure) throw failure;
        if (subscription.queue.length) { yield subscription.queue.shift(); continue; }
        await new Promise(resolve => { subscription.wake = resolve; });
      }
    } finally { signal.removeEventListener('abort', wake); subscriptions.delete(subscription); }
  }
  try {
    child.stdin.write(JSON.stringify({ root, home, providerUrl, proxyKey, model: config.model, instructions: await readFile(new URL('./instructions.md', import.meta.url), 'utf8') }) + '\n');
    const deadline = setTimeout(() => { failed(new Error(`OpenCode SDK startup timed out: ${diagnostics}`)); void close(); }, 60000);
    try { await ready; } finally { clearTimeout(deadline); }
    const session = await conversationSession(api, home);
    return { api, subscribe, session, conversation: id => conversationSession(api, home, id), close };
  } catch (error) { await close(); throw error; }
}

export async function conversationSession(api, home, id) {
  if (id && !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(id)) throw new Error('Invalid conversation');
  const path = join(home, id ? `conversation-${id}-v2.json` : 'shappy-session-v2.json');
  try {
    const saved = JSON.parse(await readFile(path, 'utf8'));
    await api(`/session/${saved.id}`);
    return saved.id;
  } catch (error) {
    if (error.code !== 'ENOENT' && error.status !== 404 && !String(error.message).includes('(404)')) throw error;
    const created = await api('/session', { title: 'Shappy' });
    await writeFile(`${path}.tmp`, JSON.stringify({ id: created.id }));
    await rename(`${path}.tmp`, path);
    return created.id;
  }
}
