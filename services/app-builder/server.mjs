import { serviceServer } from './service-server.mjs';
import { timingSafeEqual } from 'node:crypto';
import { mkdtemp, mkdir, rm, readdir, lstat } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { materialize } from './workspace.mjs';
import { code } from './agent.mjs';
import { closeEngine } from './opencode.mjs';
import { isolated, forgetDependencies } from './sandbox.mjs';

export function createBuilder(config, coding = code) {
  let active = 0;
  const workspaces = new Map();
  const activeApps = new Set();
  const server = serviceServer(async (req, res) => {
    function respond(status, body) {
      if (res.destroyed) return;
      res.writeHead(status, { 'content-type': 'application/json', 'cache-control': 'no-store' });
      res.end(JSON.stringify(body));
    }
    if (req.url === '/health' && req.method === 'GET') return respond(200, { ready: true });
    const supplied = Buffer.from(req.headers.authorization ?? '');
    const expected = Buffer.from(`Bearer ${config.token}`);
    if (supplied.length !== expected.length || !timingSafeEqual(supplied, expected)) return respond(401, { error: 'Unauthorized' });
    if (req.url !== '/build' || req.method !== 'POST') return respond(404, { error: 'Not found' });
    if (active >= config.concurrency) return respond(503, { error: 'Builder capacity reached' });
    active++;
    const controller = new AbortController();
    const deadline = setTimeout(() => controller.abort(), 900_000);
    res.on('close', () => { if (!res.writableEnded) controller.abort(); });
    let root; let appId; let heartbeat;
    let phase = 'understanding';
    let message;
    const streaming = req.headers.accept === 'application/x-ndjson';
    const emit = value => { if (!res.destroyed) res.write(JSON.stringify(value) + '\n'); };
    const onProgress = (next, text) => {
      if (!['understanding', 'editing', 'checking', 'repairing', 'waiting'].includes(next)) return;
      if (next === 'understanding' && !['understanding', 'waiting'].includes(phase)) return;
      if (phase === next && (!text || text === message)) return;
      phase = next;
      message = text;
      if (streaming) emit({ type: 'progress', phase, message });
    };
    try {
      let size = 0; const chunks = [];
      for await (const chunk of req) { size += chunk.length; if (size > 96 * 1024 * 1024) throw new Error('Request too large'); chunks.push(chunk); }
      const input = JSON.parse(Buffer.concat(chunks).toString('utf8'));
      if (!/^[a-z][a-z0-9_]{0,49}$/.test(input.appId) || typeof input.prompt !== 'string' || !input.prompt.trim() || input.prompt.length > 16000) throw new Error('Invalid application change');
      if (input.conversationId != null && !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(input.conversationId)) throw new Error('Invalid conversation');
      if (activeApps.has(input.appId)) throw new Error('An application build is already running');
      appId = input.appId; activeApps.add(appId);
      root = workspaces.get(appId);
      if (!root) {
        root = join(config.workspaceRoot ?? join(tmpdir(), 'rootcx-builder'), appId);
        await mkdir(root, { recursive: true });
        workspaces.set(appId, root);
      }
      for (const entry of await readdir(root)) {
        const path = join(root, entry);
        if (entry === 'node_modules' && (await lstat(path)).isDirectory()) continue;
        await rm(path, { recursive: true, force: true });
      }
      await materialize(root, input.files);
      if (streaming) {
        res.writeHead(200, { 'content-type': 'application/x-ndjson', 'cache-control': 'no-store', 'x-accel-buffering': 'no' });
        emit({ type: 'progress', phase });
        heartbeat = setInterval(() => emit({ type: 'progress', phase, message }), 10_000);
      }
      const result = await coding({ root, appId: input.appId, prompt: input.prompt, conversationId: input.conversationId, signal: controller.signal, config, onProgress });
      if (streaming) { emit({ type: 'result', ...result }); res.end(); }
      else respond(200, result);
    } catch (error) {
      // Diagnostics remain on the service, never in the user's business conversation.
      console.error('Application build failed:', error.message);
      const allowed = ['AI_CREDITS_EXHAUSTED', 'AI_UNAVAILABLE', 'AI_CONFIGURATION', 'AI_LIMIT_REACHED', 'BUILD_FAILED'];
      let code = 'BUILD_FAILED';
      let status = 422;
      if (controller.signal.aborted) {
        code = 'BUILD_TIMEOUT';
        status = 504;
      } else if (allowed.includes(error.code)) {
        code = error.code;
        if (code === 'AI_CREDITS_EXHAUSTED') status = 402;
      }
      if (res.headersSent) { emit({ type: 'error', error: code }); res.end(); }
      else respond(status, { error: code });
    } finally {
      clearTimeout(deadline);
      clearInterval(heartbeat);
      if (appId) activeApps.delete(appId);
      active--;
      // Bound warm workspaces; dependency caches are an optimization, never the source of truth.
      if (workspaces.size > 4) {
        for (const [id, path] of workspaces) {
          if (activeApps.has(id) || id === appId) continue;
          workspaces.delete(id);
          await closeEngine(path);
          forgetDependencies(path);
          await rm(path, { recursive: true, force: true });
          break;
        }
      }
    }
  }, config.tls);
  server.on('close', () => { for (const path of workspaces.values()) { void closeEngine(path); forgetDependencies(path); } });
  return server;
}

if (import.meta.url === new URL(process.argv[1], 'file:').href) {
  let config = {
    token: process.env.ROOTCX_BUILDER_TOKEN,
    endpoint: process.env.ROOTCX_BUILDER_LLM_ENDPOINT,
    apiKey: process.env.ROOTCX_BUILDER_LLM_KEY,
    model: process.env.ROOTCX_BUILDER_MODEL,
    concurrency: Number(process.env.ROOTCX_BUILDER_CONCURRENCY ?? 1),
    workspaceRoot: process.env.ROOTCX_BUILDER_STATE_DIR,
  };
  if (process.argv.includes('--config-stdin')) {
    const { createInterface } = await import('node:readline');
    const input = createInterface({ input: process.stdin });
    for await (const line of input) { config = { ...config, ...JSON.parse(line) }; break; }
    input.close();
  }
  if (!config.token || config.token.length < 32 || !config.apiKey || !config.model || !config.endpoint || !Number.isInteger(config.concurrency) || config.concurrency < 1 || config.concurrency > 4) throw new Error('Builder configuration incomplete');
  const endpoint = new URL(config.endpoint);
  const localHttp = process.env.ROOTCX_BUILDER_ALLOW_LOCAL_HTTP === 'true'
    && endpoint.protocol === 'http:'
    && ['localhost', '127.0.0.1', '[::1]', 'host.docker.internal'].includes(endpoint.hostname);
  if ((endpoint.protocol !== 'https:' && !localHttp) || endpoint.username || endpoint.password) throw new Error('Coding endpoint must use HTTPS');
  // Refuse readiness if the host cannot enforce the sandbox. Never silently run builds on the host.
  const probe = await mkdtemp(join(tmpdir(), 'rootcx-probe-'));
  try {
    await isolated(probe, ['node', '-e', 'process.exit(0)']);
    await isolated(probe, ['node', '-e', 'process.exit(0)'], { network: true });
  } finally {
    await rm(probe, { recursive: true, force: true });
    await rm(`${probe}.sandbox`, { recursive: true, force: true });
  }
  const server = createBuilder(config);
  server.requestTimeout = 60_000;
  server.headersTimeout = 15_000;
  server.listen(Number(process.env.PORT ?? 9201), process.env.BIND ?? '0.0.0.0', () => {
    console.info(JSON.stringify({ event: 'builder.ready', port: server.address().port }));
  });
  for (const sig of ['SIGTERM', 'SIGINT']) process.on(sig, () => { server.close(); setTimeout(() => process.exit(1), 10_000).unref(); });
}
