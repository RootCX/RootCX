import { spawn } from 'node:child_process';
import { mkdir, mkdtemp, realpath, rm } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { SandboxManager } from '@anthropic-ai/sandbox-runtime';
import { workerPolicy, allowPublicDestination } from './policy.mjs';

// Policy state and proxy authentication are private to one worker process.
const { root, prelude, args, env, storageSocket, writeRoot = false, timeoutMs } = JSON.parse(process.argv[2]);
const runtime = dirname(fileURLToPath(import.meta.url));
const home = await mkdtemp('/tmp/rootcx-worker-');
const temporary = join(home, 'tmp');
await mkdir(temporary, { mode: 0o700 });
for (const name of Object.keys(process.env)) delete process.env[name];
process.env.PATH = '/usr/local/bin:/usr/bin:/bin';
process.env.HOME = home;
process.env.TMPDIR = temporary;
process.env.CLAUDE_CODE_TMPDIR = temporary;
let child;
try {
  const dependencies = await SandboxManager.checkDependenciesAsync();
  if (dependencies.errors.length || dependencies.warnings.length) throw new Error('Sandbox dependencies unavailable');
  const readPaths = [];
  for (const path of [root, prelude, home, runtime, args[0], storageSocket, '/usr', '/bin', '/lib', '/lib64', '/etc/ssl/certs'].filter(Boolean)) {
    try { readPaths.push(await realpath(path)); } catch (error) { if (error.code !== 'ENOENT') throw error; }
  }
  const policy = workerPolicy({ readPaths, home, storageSocket });
  if (writeRoot) policy.filesystem.allowWrite.push(root);
  await SandboxManager.initialize(policy, allowPublicDestination, false);
  const quote = value => `'${String(value).replaceAll("'", "'\\''")}'`;
  const proxies = ['HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY', 'http_proxy', 'https_proxy', 'all_proxy'];
  const fixed = { PATH: '/usr/local/bin:/usr/bin:/bin', HOME: home, TMPDIR: temporary, SANDBOX_RUNTIME: '1', NO_PROXY: '', no_proxy: '', ...(storageSocket ? { ROOTCX_WORKER_STORAGE_SOCKET: storageSocket } : {}) };
  const environment = Object.entries({ ...env, ...fixed }).filter(([key]) => !proxies.includes(key));
  const exports = proxies.map(name => `${name}=\"\${${name}/localhost/127.0.0.1}\"`).join('; ');
  const proxyArgs = proxies.map(name => `\"${name}=\$${name}\"`).join(' ');
  // exec releases the readiness shell instead of retaining another Bun per app.
  const command = `cd ${quote(root)} || exit 1; ${exports}; [[ "$HTTP_PROXY" =~ :([0-9]+)/?$ ]] || exit 1; port=\"\${BASH_REMATCH[1]}\"; ready=; for i in {1..40}; do if (: > /dev/tcp/127.0.0.1/$port) 2>/dev/null; then ready=1; break; fi; sleep 0.025; done; [[ $ready == 1 ]] || exit 1; exec /usr/bin/env -i ${environment.map(([key, value]) => quote(key + '=' + value)).join(' ')} ${proxyArgs} ${args.map(quote).join(' ')}`;
  const wrapped = await SandboxManager.wrapWithSandboxArgv(command, '/bin/bash', undefined, undefined, root, { commandId: 'worker' });
  child = spawn(wrapped.argv[0], wrapped.argv.slice(1), {
    cwd: runtime, stdio: 'inherit',
    env: { PATH: '/usr/local/bin:/usr/bin:/bin', HOME: home, CI: 'true', ...wrapped.env },
  });
  const terminate = () => child.kill('SIGKILL');
  if (timeoutMs) setTimeout(terminate, timeoutMs).unref();
  process.on('SIGTERM', terminate);
  process.on('SIGINT', terminate);
  process.exitCode = await new Promise((resolve, reject) => {
    child.once('error', reject);
    child.once('exit', code => resolve(code ?? 1));
  });
} catch (error) {
  console.error(`Worker sandbox refused execution: ${error.message}`);
  process.exitCode = 1;
} finally {
  child?.kill('SIGKILL');
  SandboxManager.cleanupAfterCommand();
  await SandboxManager.reset();
  await rm(home, { recursive: true, force: true });
}
