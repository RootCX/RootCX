import { spawn } from 'node:child_process';
import { connect } from 'node:net';
import { setTimeout as delay } from 'node:timers/promises';

const { args, env } = JSON.parse(process.argv[2]);
for (const name of ['HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY', 'http_proxy', 'https_proxy', 'all_proxy']) {
  if (!process.env[name]) continue;
  const endpoint = new URL(process.env[name]);
  endpoint.hostname = '127.0.0.1';
  process.env[name] = endpoint.href;
}
const proxy = new URL(process.env.HTTP_PROXY);
for (let attempt = 0; ; attempt++) {
  try {
    await new Promise((resolve, reject) => {
      const socket = connect({ host: proxy.hostname, port: Number(proxy.port) });
      socket.once('connect', () => { socket.destroy(); resolve(); });
      socket.once('error', reject);
      socket.setTimeout(100, () => { socket.destroy(); reject(new Error('Proxy readiness timeout')); });
    });
    break;
  } catch (error) { if (attempt >= 20) throw error; await delay(25); }
}
const child = spawn(args[0], args.slice(1), { stdio: 'inherit', env: { ...process.env, ...env, NO_PROXY: '', no_proxy: '' } });
child.once('error', error => { console.error(error.message); process.exitCode = 1; });
child.once('exit', code => { process.exitCode = code ?? 1; });
