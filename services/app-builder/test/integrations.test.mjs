import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile, readdir } from 'node:fs/promises';
import { integrationCatalog } from '../scripts/package-integrations.mjs';

const directory = new URL('../../../core/resources/integrations/', import.meta.url);
async function manifests() {
  const entries = await readdir(directory, { withFileTypes: true });
  return Promise.all(entries.filter(entry => entry.isDirectory()).map(async entry =>
    JSON.parse(await readFile(new URL(`${entry.name}/manifest.json`, directory), 'utf8'))));
}

test('catalog lists the real integration and action IDs the agent must use', async () => {
  const catalog = integrationCatalog(await manifests());
  for (const id of ['gmail', 'google_calendar', 'google_sheets', 'google_drive', 'notion', 'stripe']) {
    assert.match(catalog, new RegExp(`### \`${id}\``));
  }
  for (const action of ['live_search_messages', 'live_get_message', 'live_create_draft', 'live_list_events', 'list_spreadsheets', 'read_range', 'append_rows', 'list_files']) {
    assert.match(catalog, new RegExp(`\`${action}\`\\(`));
  }
  assert.match(catalog, /spreadsheetId: string, range: string/);
});

test('catalog hides plumbing and legacy sync actions superseded by live actions', async () => {
  const catalog = integrationCatalog(await manifests());
  for (const action of ['sync_connect', 'watch', 'verify_connection', 'live_verify_connection']) {
    assert.doesNotMatch(catalog, new RegExp(`- \`${action}\`\\(`));
  }
  const section = id => catalog.split(`### \`${id}\``)[1].split('\n### ')[0];
  assert.doesNotMatch(section('gmail'), /- `send_email`\(/);
  assert.doesNotMatch(section('google_calendar'), /- `list_events`\(/);
});
