import { mkdir, readFile, readdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';

const [skillDirectory = '/opt/rootcx-skills/rootcx', output = '/opt/rootcx-docs'] = process.argv.slice(2);
const urls = new Set();
async function scan(directory) {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) await scan(path);
    else if (entry.isFile() && entry.name.endsWith('.md')) {
      for (const match of (await readFile(path, 'utf8')).matchAll(/https:\/\/rootcx\.com\/docs\/[a-z0-9_/-]+\.md/g)) urls.add(match[0]);
    }
  }
}
await scan(skillDirectory);
if (urls.size < 9) throw new Error('RootCX skill documentation index is incomplete');
for (const url of [...urls].sort()) {
  const response = await fetch(url, { redirect: 'error', signal: AbortSignal.timeout(30_000) });
  if (!response.ok) throw new Error(`Documentation unavailable (${response.status}): ${url}`);
  const body = await response.text();
  if (!body.trim() || body.length > 2_000_000 || /^\s*<!doctype html/i.test(body)) throw new Error(`Invalid documentation: ${url}`);
  const path = join(output, new URL(url).pathname.slice('/docs/'.length));
  await mkdir(join(path, '..'), { recursive: true });
  await writeFile(path, body);
}
await writeFile(join(output, 'INDEX.md'), [...urls].sort().map(url => `- ${url} → ${new URL(url).pathname.slice('/docs/'.length)}`).join('\n') + '\n');
