// Run against a disposable local instance or an explicitly selected EKS test tenant.
import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { setTimeout as delay } from 'node:timers/promises';

const { ROOTCX_SMOKE_URL: origin, ROOTCX_SMOKE_TOKEN: token } = process.env;
if (!origin || !token || !process.argv[2]) throw new Error('Set ROOTCX_SMOKE_URL and ROOTCX_SMOKE_TOKEN; pass an exported template sources.json');
const appId = `release_smoke_${randomUUID().replaceAll('-', '').slice(0, 12)}`;
const marker = `Verified ${randomUUID()}`;
const { files } = JSON.parse(await readFile(process.argv[2], 'utf8'));
const manifest = JSON.parse(Buffer.from(files['manifest.json'], 'base64').toString());
const templateId = manifest.appId;
if (!templateId) throw new Error('Template manifest lacks appId');
for (const [path, encoded] of Object.entries(files)) {
  const bytes = Buffer.from(encoded, 'base64');
  const text = bytes.toString('utf8');
  if (Buffer.from(text).equals(bytes)) files[path] = Buffer.from(text.replaceAll(templateId, appId)).toString('base64');
}
const smokeManifest = JSON.parse(Buffer.from(files['manifest.json'], 'base64').toString());
smokeManifest.dataContract = [...(smokeManifest.dataContract ?? []), { entityName: 'release_checks', fields: [{ name: 'label', type: 'text' }] }];
files['manifest.json'] = Buffer.from(JSON.stringify(smokeManifest)).toString('base64');
async function api(path, body) {
  const response = await fetch(`${origin.replace(/\/$/, '')}${path}`, {
    method: body ? 'POST' : 'GET',
    headers: { authorization: `Bearer ${token}`, 'content-type': 'application/json' },
    body: body ? JSON.stringify(body) : undefined, signal: AbortSignal.timeout(120_000),
  });
  if (!response.ok) throw new Error(`Smoke request failed (${response.status}): ${path}`);
  return response.json();
}
await api('/api/v1/apps', smokeManifest);
const base = `/api/v1/apps/${appId}`;
const imported = await api(`${base}/sources`, { files });
console.log(`Test application: ${appId}`);
const request = {
  requestId: randomUUID(), baseCommit: imported.headCommit,
  prompt: `Dans cette application de test, ajoute un champ texte facultatif « Note de vérification » (nom technique release_note) à la collection release_checks et affiche le texte exact « ${marker} » sur la page d'accueil.`,
};
const run = await api(`${base}/changes`, request);
assert.equal((await api(`${base}/changes`, request)).id, run.id, 'retry created another request');
const deadline = Date.now() + 16 * 60_000;
let final;
let phase;
while (Date.now() < deadline) {
  final = await api(`${base}/changes/${run.id}`);
  if (phase !== final.phase) {
    phase = final.phase;
    console.log(`Progress: ${phase}`);
  }
  if (!['queued', 'coding', 'publishing'].includes(final.status)) break;
  await delay(1000);
}
assert.equal(final.status, 'succeeded', `Release failed: ${final.status} (${final.errorCode ?? 'see private run diagnostics'})`);
assert.ok(final.commitId, 'No published revision');
const source = await api(`${base}/sources`);
assert.equal(source.headCommit, final.commitId);
const entity = 'release_checks';
// The schema must accept the new field; a frontend-only edit cannot pass this check.
const record = await api(`${base}/collections/${entity}`, { release_note: marker });
assert.equal(record.release_note, marker);
const response = await fetch(`${origin}/apps/${appId}/`, { signal: AbortSignal.timeout(15_000) });
assert.equal(response.status, 200);
const html = await response.text();
const scripts = [...html.matchAll(/src=["']([^"']+\.js)["']/g)].map(match => new URL(match[1], response.url));
const assets = await Promise.all(scripts.map(async url => {
  assert.equal(url.origin, new URL(origin).origin);
  const asset = await fetch(url, { signal: AbortSignal.timeout(15_000) });
  assert.equal(asset.status, 200);
  return asset.text();
}));
assert.ok([html, ...assets].some(text => text.includes(marker)), 'Published frontend lacks the requested marker');
console.log(`PASS: published revision ${final.commitId}; schema and served frontend verified.\nRetained for inspection: ${origin}/apps/${appId}/`);
