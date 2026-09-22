// Development launcher: receive credentials over stdin, never Docker env or disk.
import { createInterface } from 'node:readline';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { isolated } from './sandbox.mjs';
import { createBuilder } from './server.mjs';
const lines = createInterface({ input: process.stdin });
const line = await new Promise(resolve => lines.once('line', resolve));
lines.close();
const config = JSON.parse(line);
const endpoint = new URL(config.endpoint);
if (!config.token || config.token.length < 32 || !config.apiKey || !config.model ||
    endpoint.username || endpoint.password ||
    !(endpoint.protocol === 'https:' || (endpoint.protocol === 'http:' && endpoint.hostname === 'host.docker.internal'))) {
  throw new Error('Invalid local builder configuration');
}
config.concurrency = 1;
const probe = await mkdtemp(join(tmpdir(), 'rootcx-probe-'));
try { await isolated(probe, ['node', '-e', 'process.exit(0)']); }
finally { await rm(probe, { recursive: true, force: true }); }
const server = createBuilder(config);
server.requestTimeout = 60_000;
server.headersTimeout = 15_000;
server.listen(9201, '0.0.0.0', () => console.log('Local builder ready'));
for (const signal of ['SIGTERM', 'SIGINT']) process.on(signal, () => server.close());
