import { plugins } from './src/registry';
const [id, connection] = process.argv.slice(2);
const plugin = plugins.find(p => p.id === id);
if (!plugin || !connection || !/^[A-Za-z0-9_-]+$/.test(connection)) throw new Error('Usage: bun live-check.ts <provider> <connection-id>');
const token = process.env.ROOTCX_ACCESS_TOKEN;
if (!token) throw new Error('Set ROOTCX_ACCESS_TOKEN in your environment');
const base = new URL(process.env.ROOTCX_CORE_URL ?? '');
if (base.username || base.password || base.search || base.hash || (base.protocol !== 'https:' && !['localhost', '127.0.0.1'].includes(base.hostname))) throw new Error('Use an HTTPS Core URL (or local test Core)');
const action = ['gmail', 'google_calendar'].includes(id) ? 'live_verify_connection' : 'verify_connection';
try {
  const response = await fetch(new URL(`/api/v1/integrations/${id}/actions/${action}`, base), {
    method: 'POST', redirect: 'error', signal: AbortSignal.timeout(30000),
    headers: { authorization: `Bearer ${token}`, 'Content-Type': 'application/json', 'x-integration-connection': connection }, body: '{}',
  });
  const result = await response.json() as { ok?: boolean; error?: { code?: string } };
  const passed = response.ok && result.ok === true;
  // Deliberately omit account identity, response data, tokens and raw errors.
  console.log(JSON.stringify({ provider: id, version: plugin.version, checkedAt: new Date().toISOString(), check: 'live-account-access-only', passed, httpStatus: response.status }));
  if (!passed) process.exitCode = 1;
} catch { console.error('Live account check could not be confirmed; inspect Core diagnostics without exposing secrets.'); process.exitCode = 1; }
