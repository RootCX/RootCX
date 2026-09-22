import { createServer } from 'node:http';
import { timingSafeEqual } from 'node:crypto';
import { mkdtemp, rm, readdir, lstat } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { materialize } from './workspace.mjs';
import { code } from './agent.mjs';
import { isolated, forgetDependencies } from './sandbox.mjs';

export function createBuilder(config, coding = code) {
  let active = 0;
  const workspaces = new Map();
  const activeApps = new Set();
  const server = createServer(async (req, res) => {
    const respond = (status, body) => { if (!res.destroyed) { res.writeHead(status, { 'content-type': 'application/json', 'cache-control': 'no-store' }); res.end(JSON.stringify(body)); } };
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
    let root; let appId;
    try {
      let size = 0; const chunks = [];
      for await (const chunk of req) { size += chunk.length; if (size > 96 * 1024 * 1024) throw new Error('Request too large'); chunks.push(chunk); }
      const input = JSON.parse(Buffer.concat(chunks).toString('utf8'));
      if (!/^[a-z][a-z0-9_]{0,49}$/.test(input.appId) || typeof input.prompt !== 'string' || !input.prompt.trim() || input.prompt.length > 16000) throw new Error('Invalid application change');
      if (activeApps.has(input.appId)) throw new Error('An application build is already running');
      appId = input.appId; activeApps.add(appId);
      root = workspaces.get(appId);
      if (!root) {
        root = await mkdtemp(join(config.workspaceRoot ?? tmpdir(), 'rootcx-build-'));
        workspaces.set(appId, root);
      }
      for (const entry of await readdir(root)) {
        const path = join(root, entry);
        if (entry === 'node_modules' && (await lstat(path)).isDirectory()) continue;
        await rm(path, { recursive: true, force: true });
      }
      await materialize(root, input.files);
      const result = await coding({ root, appId: input.appId, prompt: input.prompt, signal: controller.signal, config });
      respond(200, result);
    } catch (error) {
      // Diagnostics remain on the service, never in the user's business conversation.
      console.error('Application build failed:', error.message);
      if (error.code === 'AI_CREDITS_EXHAUSTED') respond(402, { error: 'AI_CREDITS_EXHAUSTED' });
      else respond(controller.signal.aborted ? 504 : 422, { error: 'Application build failed' });
    } finally {
      clearTimeout(deadline);
      if (appId) activeApps.delete(appId);
      active--;
      // Bound warm workspaces; dependency caches are an optimization, never the source of truth.
      if (workspaces.size > 4) {
        for (const [id, path] of workspaces) {
          if (!activeApps.has(id) && id !== appId) { workspaces.delete(id); forgetDependencies(path); await rm(path, { recursive: true, force: true }); break; }
        }
      }
    }
  });
  server.on('close', () => { for (const path of workspaces.values()) { forgetDependencies(path); void rm(path, { recursive: true, force: true }); } });
  return server;
}

if (import.meta.url === new URL(process.argv[1], 'file:').href) {
  const config = {
    token: process.env.ROOTCX_BUILDER_TOKEN,
    endpoint: process.env.ROOTCX_BUILDER_LLM_ENDPOINT,
    apiKey: process.env.ROOTCX_BUILDER_LLM_KEY,
    model: process.env.ROOTCX_BUILDER_MODEL,
    concurrency: Number(process.env.ROOTCX_BUILDER_CONCURRENCY ?? 1),
  };
  if (!config.token || config.token.length < 32 || !config.apiKey || !config.model || !config.endpoint || !Number.isInteger(config.concurrency) || config.concurrency < 1 || config.concurrency > 4) throw new Error('Builder configuration incomplete');
  const endpoint = new URL(config.endpoint);
  if (endpoint.protocol !== 'https:' || endpoint.username || endpoint.password) throw new Error('Coding endpoint must use HTTPS');
  // Refuse readiness if the host cannot enforce the sandbox. Never silently run builds on the host.
  const probe = await mkdtemp(join(tmpdir(), 'rootcx-probe-'));
  try { await isolated(probe, ['node', '-e', 'process.exit(0)']); } finally { await rm(probe, { recursive: true, force: true }); }
  const server = createBuilder(config);
  server.requestTimeout = 60_000;
  server.headersTimeout = 15_000;
  server.listen(Number(process.env.PORT ?? 9201), process.env.BIND ?? '0.0.0.0');
  for (const sig of ['SIGTERM', 'SIGINT']) process.on(sig, () => { server.close(); setTimeout(() => process.exit(1), 10_000).unref(); });
}
