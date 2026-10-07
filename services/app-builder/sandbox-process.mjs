import { spawn } from 'node:child_process';
import { mkdir, realpath } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';

export async function spawnSandbox(root, args, { env = {}, home = `${root}.sandbox`, providerUrl, stdio = ['pipe', 'pipe', 'pipe'] } = {}) {
  if (process.platform !== 'linux') throw new Error('The production builder requires Linux isolation; no unsandboxed fallback');
  await mkdir(home, { recursive: true, mode: 0o700 });
  root = await realpath(root);
  home = await realpath(home);
  const options = { root, home, args, env, providerUrl };
  return spawn(process.execPath, [fileURLToPath(new URL('./sandbox-supervisor.mjs', import.meta.url)), JSON.stringify(options)], {
    detached: true, stdio, cwd: root,
    env: { PATH: '/usr/local/bin:/usr/bin:/bin', HOME: home },
  });
}
