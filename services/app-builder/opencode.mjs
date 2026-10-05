import { spawn } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { access, mkdir, readFile, rename, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { createProviderProxy } from './provider-proxy.mjs';

const engines = new Map();

export function providerError(error) {
  let body;
  try { body = JSON.parse(error?.data?.responseBody); } catch {}
  if (body?.error?.code === 'AI_CONFIGURATION') return 'AI_CONFIGURATION';
  const status = error?.data?.statusCode;
  if (status === 402) return 'AI_CREDITS_EXHAUSTED';
  if ([401, 403].includes(status) || error?.name === 'ProviderAuthError') return 'AI_CONFIGURATION';
  if (status === 429 || status >= 500) return 'AI_UNAVAILABLE';
  if (['ContextOverflowError', 'MessageOutputLengthError'].includes(error?.name)) return 'AI_LIMIT_REACHED';
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

export async function* events(body) {
  const decoder = new TextDecoder();
  let pending = '';
  for await (const chunk of body) {
    pending += decoder.decode(chunk, { stream: true }).replaceAll('\r\n', '\n');
    if (pending.length > 2_000_000) throw new Error('OpenCode event exceeds limit');
    let index;
    while ((index = pending.indexOf('\n\n')) !== -1) {
      const block = pending.slice(0, index); pending = pending.slice(index + 2);
      const data = block.split('\n').filter(line => line.startsWith('data:')).map(line => line.slice(5).trimStart()).join('\n');
      if (data) yield JSON.parse(data);
    }
  }
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
  runtime?.close();
}

async function startEngine(root, config) {
  const home = `${root}.agent`;
  await mkdir(home, { recursive: true });
  const proxyKey = randomBytes(32).toString('hex');
  // The real provider key stays in this process, outside the coding environment.
  const proxy = createProviderProxy(config, proxyKey);
  await new Promise(resolve => proxy.listen(0, '127.0.0.1', resolve));
  // Reserve an ephemeral loopback port; OpenCode is internal to this runner.
  const reservation = createServer();
  await new Promise(resolve => reservation.listen(0, '127.0.0.1', resolve));
  const port = reservation.address().port;
  await new Promise(resolve => reservation.close(resolve));
  const password = randomBytes(32).toString('hex');
  const settings = {
    model: `anthropic/${config.model}`, small_model: `anthropic/${config.model}`,
    share: 'disabled', autoupdate: false, permission: 'allow',
    enabled_providers: ['anthropic'],
    provider: { anthropic: { options: { baseURL: `http://127.0.0.1:${proxy.address().port}/v1`, apiKey: proxyKey }, models: { [config.model]: { name: config.model, limit: { context: 200000, output: 16000 } } } } },
    skills: { paths: ['/opt/rootcx-skills'] },
    agent: { build: { prompt: await readFile(new URL('./instructions.md', import.meta.url), 'utf8'), tools: { question: false }, steps: 80 } },
  };
  const extraMounts = [];
  try { await access('/lib64'); extraMounts.push('--ro-bind', '/lib64', '/lib64'); } catch (error) { if (error.code !== 'ENOENT') throw error; }
  const args = ['--die-with-parent', '--new-session', '--unshare-user', '--unshare-pid', '--unshare-ipc', '--unshare-uts', '--clearenv',
    '--ro-bind', '/usr', '/usr', '--ro-bind', '/bin', '/bin', '--ro-bind', '/lib', '/lib', ...extraMounts,
    '--ro-bind', '/etc/ssl/certs', '/etc/ssl/certs', '--ro-bind', '/etc/resolv.conf', '/etc/resolv.conf',
    '--ro-bind', '/opt/rootcx-skills', '/opt/rootcx-skills',
    '--proc', '/proc', '--dev', '/dev', '--tmpfs', '/tmp', '--bind', root, '/workspace', '--bind', home, '/home/agent', '--chdir', '/workspace',
    '--setenv', 'PATH', '/usr/local/bin:/usr/bin:/bin', '--setenv', 'HOME', '/home/agent', '--setenv', 'CI', 'true',
    '--setenv', 'OPENCODE_CONFIG_CONTENT', JSON.stringify(settings), '--setenv', 'OPENCODE_SERVER_PASSWORD', password,
    '--setenv', 'OPENCODE_DISABLE_AUTOUPDATE', 'true', '--setenv', 'OPENCODE_DISABLE_SHARE', 'true',
    '--', 'opencode', 'serve', '--hostname', '127.0.0.1', '--port', String(port)];
  const child = spawn('bwrap', args, { detached: true, stdio: ['ignore', 'pipe', 'pipe'], env: { PATH: process.env.PATH } });
  let diagnostics = '';
  let exit;
  function recordDiagnostics(chunk) {
    diagnostics = (diagnostics + chunk).slice(-8000);
  }
  child.stdout.on('data', recordDiagnostics);
  child.stderr.on('data', recordDiagnostics);
  child.on('exit', code => { exit = code ?? -1; engines.delete(root); proxy.close(); });
  child.on('error', () => { exit = -1; });
  const headers = { authorization: `Basic ${Buffer.from(`opencode:${password}`).toString('base64')}`, 'content-type': 'application/json' };
  const url = `http://127.0.0.1:${port}`;
  function close() {
    try { process.kill(-child.pid, 'SIGKILL'); } catch {}
    proxy.closeAllConnections();
    proxy.close();
  }
  try {
    for (let n = 0; ; n++) {
      if (exit !== undefined || n > 120) throw new Error(`OpenCode startup failed: ${diagnostics}`);
      try { if ((await fetch(`${url}/global/health`, { headers, signal: AbortSignal.timeout(500) })).ok) break; } catch {}
      await delay(250);
    }
    async function api(path, body, signal) {
      const response = await fetch(url + path, { method: body === undefined ? 'GET' : 'POST', headers, body: body === undefined ? undefined : JSON.stringify(body), signal });
      if (!response.ok) throw new Error(`OpenCode request failed (${response.status}): ${(await response.text()).slice(0, 1000)}`);
      return response.json();
    }
    let session;
    const sessionFile = join(home, 'shappy-session.json');
    try {
      session = JSON.parse(await readFile(sessionFile, 'utf8'));
      await api(`/session/${session.id}`);
    } catch {
      session = await api('/session', { title: 'Shappy' });
      await writeFile(sessionFile, JSON.stringify({ id: session.id }));
    }
    return { api, url, headers, session: session.id, conversation: id => id ? conversationSession(api, home, id) : Promise.resolve(session.id), close };
  } catch (error) {
    close();
    throw error;
  }
}

export async function conversationSession(api, home, id) {
  if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(id)) throw new Error('Invalid conversation');
  const path = join(home, `conversation-${id}.json`);
  try {
    const saved = JSON.parse(await readFile(path, 'utf8'));
    await api(`/session/${saved.id}`);
    return saved.id;
  } catch (error) {
    if (error.code !== 'ENOENT' && !String(error.message).includes('(404)')) throw error;
    const created = await api('/session', { title: 'Shappy' });
    await writeFile(`${path}.tmp`, JSON.stringify({ id: created.id }));
    await rename(`${path}.tmp`, path);
    return created.id;
  }
}
