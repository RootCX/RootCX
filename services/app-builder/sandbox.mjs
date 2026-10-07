import { spawn } from 'node:child_process';
import { access, mkdir, mkdtemp, rm, readFile, readdir, lstat } from 'node:fs/promises';
import { join } from 'node:path';
import { createHash } from 'node:crypto';
import { tmpdir } from 'node:os';
import { confined, collect } from './workspace.mjs';
import { spawnSandbox } from './sandbox-process.mjs';
export { spawnSandbox } from './sandbox-process.mjs';
const installedLocks = new Map();
export function forgetDependencies(root) { installedLocks.delete(root); installedLocks.delete(join(root, 'backend')); }

export function command(executable, args, options = {}) {
  return commandResult(spawn(executable, args, { ...options, detached: true, stdio: ['ignore', 'pipe', 'pipe'], env: { PATH: process.env.PATH } }), options);
}

function commandResult(child, options = {}) {
  return new Promise((resolve, reject) => {
    let output = ''; let failed = false;
    function stop(error) {
      if (failed) return;
      failed = true;
      try { process.kill(-child.pid, 'SIGKILL'); } catch {}
      reject(error);
    }
    const timeout = setTimeout(() => stop(new Error('Command timed out')), options.timeout ?? 180_000);
    const abort = () => stop(new Error('Build cancelled'));
    options.signal?.addEventListener('abort', abort, { once: true });
    if (options.signal?.aborted) abort();
    for (const stream of [child.stdout, child.stderr]) stream.on('data', chunk => {
      output += chunk.toString();
      if (output.length > 1_000_000) stop(new Error('Command output limit exceeded'));
    });
    child.on('error', stop);
    child.on('close', code => {
      clearTimeout(timeout); options.signal?.removeEventListener('abort', abort);
      if (!failed) code === 0 ? resolve(output) : reject(new Error(output.slice(-24000) || `Command failed (${code})`));
    });
  });
}

export async function isolated(root, args, { network = false, signal } = {}) {
  if (process.platform !== 'linux') throw new Error('The production builder requires Linux bubblewrap; no unsandboxed fallback');
  if (network) return commandResult(await spawnSandbox(root, args, {
    stdio: ['ignore', 'pipe', 'pipe'],
    env: { npm_config_cache: `${root}.sandbox/npm-cache`, npm_config_registry: 'https://registry.npmjs.org' },
  }), { signal });
  const mounts = [];
  for (const path of ['/usr', '/bin', '/lib', '/lib64', '/etc/ssl/certs', '/etc/resolv.conf']) {
    try { await access(path); mounts.push('--ro-bind', path, path); } catch {}
  }
  const dependencies = [];
  if (!network) {
    for (const relative of ['node_modules', 'backend/node_modules']) {
      try {
        const path = join(root, relative);
        if (!(await lstat(path)).isDirectory()) throw new Error('Dependencies must be real directories');
        dependencies.push('--ro-bind', path, `/workspace/${relative}`);
        if (relative === 'node_modules') {
          const temporary = join(path, '.vite-temp');
          await mkdir(temporary, { recursive: true });
          if (!(await lstat(temporary)).isDirectory()) throw new Error('Invalid Vite temporary directory');
          dependencies.push('--tmpfs', '/workspace/node_modules/.vite-temp');
        }
      } catch (error) { if (error.code !== 'ENOENT') throw error; }
    }
  }
  return command('bwrap', [
    '--die-with-parent', '--new-session', '--unshare-user', '--unshare-pid', '--unshare-ipc', '--unshare-uts',
    ...(network ? [] : ['--unshare-net']), '--clearenv', ...mounts,
    '--proc', '/proc', '--dev', '/dev', '--tmpfs', '/tmp', '--dir', '/home',
    '--bind', root, '/workspace', ...dependencies, '--chdir', '/workspace',
    '--setenv', 'PATH', '/usr/local/bin:/usr/bin:/bin', '--setenv', 'HOME', '/tmp',
    '--setenv', 'CI', 'true', '--setenv', 'npm_config_cache', '/tmp/npm-cache',
    '--', ...args,
  ], { signal });
}

export async function build(root, appId, signal) {
  const pkg = JSON.parse(await readFile(await confined(root, 'package.json'), 'utf8'));
  if (!pkg.devDependencies?.vite && !pkg.dependencies?.vite) throw new Error('RootCX frontend must declare Vite');
  await installDependencies(root, '', signal);
  let backendPackage = false;
  try { await access(await confined(root, 'backend/package.json')); backendPackage = true; }
  catch (error) { if (error.code !== 'ENOENT') throw error; }
  if (backendPackage) await installDependencies(root, 'backend', signal);
  await isolated(root, ['node', 'node_modules/typescript/bin/tsc', '--noEmit'], { signal });
  try {
    await access(await confined(root, 'backend/index.ts'));
    await isolated(root, ['node', 'node_modules/typescript/bin/tsc', '--noEmit', '--skipLibCheck', '--target', 'ES2022', '--module', 'preserve', '--moduleResolution', 'bundler', 'backend/index.ts'], { signal });
    await isolated(root, ['bun', 'build', 'backend/index.ts', '--target=bun', '--packages=external', '--outfile=/tmp/rootcx-worker.js'], { signal });
  } catch (error) { if (error.code !== 'ENOENT') throw error; }
  if (pkg.scripts?.test) await isolated(root, ['npm', 'test', '--', '--run'], { signal });
  else if (Object.keys(await collect(root)).some(path => /(?:^|\/)backend\/.*\.test\.ts$/.test(path))) await isolated(root, ['bun', 'test', 'backend'], { signal });
  await isolated(root, ['node', 'node_modules/vite/bin/vite.js', 'build', '--base', `/apps/${appId}/`], { signal });
  // Archive traversal rejects links rather than dereferencing arbitrary files.
  const paths = [];
  async function walk(prefix) {
    for (const entry of await readdir(join(root, 'dist', prefix), { withFileTypes: true })) {
      const name = prefix ? `${prefix}/${entry.name}` : entry.name;
      if (entry.isSymbolicLink() || (!entry.isDirectory() && !entry.isFile())) throw new Error('Frontend artifact contains a link or special file');
      if (entry.isDirectory()) await walk(name); else paths.push(name);
    }
  }
  if (!(await lstat(join(root, 'dist'))).isDirectory()) throw new Error('Frontend output must be a real directory');
  await walk('');
  if (!paths.includes('index.html')) throw new Error('Build did not produce index.html');
  let size = 0;
  for (const path of paths) size += (await lstat(join(root, 'dist', path))).size;
  if (paths.length > 10000 || size > 128 * 1024 * 1024) throw new Error('Frontend artifact too large');
  const output = await mkdtemp(join(tmpdir(), 'rootcx-artifact-'));
  try {
    const archive = join(output, 'frontend.tar.gz');
    await command('tar', ['-czf', archive, '-C', join(root, 'dist'), '--', ...paths], { signal });
    const bytes = await readFile(archive);
    if (bytes.length > 50 * 1024 * 1024) throw new Error('Frontend artifact too large');
    return bytes.toString('base64');
  } finally { await rm(output, { recursive: true, force: true }); }
}

async function installDependencies(root, relative, signal) {
  const directory = join(root, relative);
  const pkg = await readFile(await confined(root, join(relative, 'package.json')));
  let manager = 'npm';
  let lock;
  try { lock = await readFile(await confined(root, join(relative, 'bun.lock'))); manager = 'bun'; }
  catch (error) { if (error.code !== 'ENOENT') throw error; lock = await readFile(await confined(root, join(relative, 'package-lock.json'))); }
  const digest = createHash('sha256').update(pkg).update(lock).digest('hex');
  let present = false;
  try { present = (await lstat(join(directory, 'node_modules'))).isDirectory(); } catch (error) { if (error.code !== 'ENOENT') throw error; }
  if (installedLocks.get(directory) === digest && present) return;
  const args = manager === 'bun'
    ? ['bun', ...(relative ? ['--cwd', relative] : []), 'install', '--frozen-lockfile', '--ignore-scripts']
    : ['npm', ...(relative ? ['--prefix', relative] : []), 'ci', '--ignore-scripts', '--no-audit', '--no-fund'];
  await isolated(root, args, { network: true, signal });
  installedLocks.set(directory, digest);
}
