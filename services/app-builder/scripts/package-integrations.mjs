import { readFile, readdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';

// Plumbing every connector shares; apps never call it directly.
const hidden = /^(sync_|watch$|stop_watch$|verify_connection$|live_verify_connection$|api_read$|api_write$)/;

function parameters(schema) {
  const properties = schema?.properties ?? {};
  const required = new Set(schema?.required ?? []);
  return Object.entries(properties)
    .map(([name, spec]) => `${name}${required.has(name) ? '' : '?'}: ${Array.isArray(spec.type) ? spec.type.join('|') : spec.type ?? 'any'}`)
    .join(', ');
}

export function integrationCatalog(manifests) {
  const sections = manifests
    .filter(manifest => manifest.type === 'integration' || manifest.userAuth || manifest.actions?.length)
    .sort((a, b) => (a.appId ?? '').localeCompare(b.appId ?? ''))
    .map(manifest => {
      const live = manifest.userAuth?.plugin?.legacySync === true;
      const actions = (manifest.actions ?? [])
        .filter(action => !hidden.test(action.id) && (!live || action.id.startsWith('live_')))
        .map(action => `- \`${action.id}\`(${parameters(action.inputSchema)}): ${action.description || action.name || ''}`.trim());
      return [`### \`${manifest.appId}\` — ${manifest.name}`, ...actions].join('\n');
    });
  return `# Integrations available to applications

Use only these integration IDs and action IDs; never guess others. In the UI, call
\`const x = useIntegration("<id>")\` from \`@rootcx/sdk\`, then \`await x.call("<action>", input)\`.
\`call\` resolves to the envelope \`{ ok: boolean, data?: unknown, error?: { code, message } }\`:
check \`ok\` and read \`data\`. \`x.connected\` is true once the current user has connected the
integration; otherwise show a button calling \`x.connect()\`. Each user connects their own account.

Responses verified against live providers:
- List actions return \`data = { items: [...], nextCursor }\`; pass \`cursor: nextCursor\` for the next page.
- \`gmail\` \`live_search_messages\` items are \`{ id, threadId }\` only; call \`live_get_message\` for each id.
  A message has \`snippet\`, \`internalDate\` and \`payload.headers\` (\`[{ name, value }]\` with From, To, Subject, Date).
  \`query\` uses Gmail search syntax, for example \`subject:"Quote request"\`.
- \`google_calendar\` \`live_list_calendars\` items have \`id\` and \`summary\`; use \`primary\` for the user's calendar.
  \`live_list_events\` items have \`id, summary, description, location, start, end, htmlLink\`;
  \`start\`/\`end\` are \`{ dateTime, timeZone }\` or \`{ date }\` for all-day events.
- \`google_sheets\` \`list_spreadsheets\` items are \`{ id, name }\`; find a spreadsheet by \`name\`, then use its \`id\`
  as \`spreadsheetId\`. \`read_range\` returns \`{ range, values: string[][] }\` (first row = headers);
  \`append_rows\` takes \`values: string[][]\`. Ranges use A1 notation, for example \`A1:C100\`.
- \`google_drive\` \`list_files\` items are \`{ id, name, mimeType, modifiedTime, size, webViewLink }\`;
  \`query\` uses Drive search syntax, for example \`name contains 'Dupont'\` (plain words are rejected).

${sections.join('\n\n')}
`;
}

if (import.meta.url === new URL(process.argv[1], 'file:').href) {
  const [directory, output] = process.argv.slice(2);
  if (!directory || !output) throw new Error('Usage: package-integrations.mjs <integrations-dir> <output.md>');
  const manifests = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue;
    try { manifests.push(JSON.parse(await readFile(join(directory, entry.name, 'manifest.json'), 'utf8'))); }
    catch (error) { if (error.code !== 'ENOENT') throw error; }
  }
  if (manifests.length < 9) throw new Error('Integration catalog is incomplete');
  await writeFile(output, integrationCatalog(manifests));
}
