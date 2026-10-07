import { spawn } from 'node:child_process';
import { mkdir, realpath } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { SandboxManager } from '@anthropic-ai/sandbox-runtime';
import { sandboxPolicy } from './sandbox-policy.mjs';

// SRT owns module-global policy. Each application gets a separate supervisor.
const { root, home, args, env, providerUrl } = JSON.parse(process.argv[2]);
const temporary = join(home, 'tmp');
await mkdir(temporary, { recursive: true, mode: 0o700 });
process.env.TMPDIR = temporary;
process.env.CLAUDE_CODE_TMPDIR = temporary;
const runtime = dirname(fileURLToPath(import.meta.url));
const readPaths = [root, home, runtime, '/usr', '/bin', '/lib', '/lib64', '/etc/ssl/certs', '/opt/rootcx-skills', '/opt/rootcx-docs'];
const paths = [];
for (const path of readPaths) {
  try { paths.push(await realpath(path)); } catch (error) { if (error.code !== 'ENOENT') throw error; }
}
let child;
try {
  const dependencies = await SandboxManager.checkDependenciesAsync();
  if (dependencies.errors.length || dependencies.warnings.length) {
    throw new Error(`Sandbox dependencies unavailable: ${[...dependencies.errors, ...dependencies.warnings].join('; ')}`);
  }
  await SandboxManager.initialize(sandboxPolicy({ root, home, readPaths: paths, providerUrl }), undefined, false);
  const quote = value => `'${String(value).replaceAll("'", "'\\''")}'`;
  const command = ['node', join(runtime, 'sandbox-entry.mjs'), JSON.stringify({ args, env })].map(quote).join(' ');
  const wrapped = await SandboxManager.wrapWithSandboxArgv(command, '/bin/bash', undefined, undefined, root);
  child = spawn(wrapped.argv[0], wrapped.argv.slice(1), {
    cwd: root, stdio: 'inherit',
    env: { PATH: '/usr/local/bin:/usr/bin:/bin', HOME: home, CI: 'true', ...wrapped.env },
  });
  const terminate = () => child.kill('SIGKILL');
  process.on('SIGTERM', terminate);
  process.on('SIGINT', terminate);
  process.exitCode = await new Promise((resolve, reject) => {
    child.once('error', reject);
    child.once('exit', code => resolve(code ?? 1));
  });
} catch (error) {
  console.error(`Sandbox refused execution: ${error.message}`);
  process.exitCode = 1;
} finally {
  child?.kill('SIGKILL');
  SandboxManager.cleanupAfterCommand();
  await SandboxManager.reset();
}
