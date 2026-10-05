// Operator/template publishing utility. End users never need a CLI or local sources.
import { realpath, writeFile } from 'node:fs/promises';
import { collect, validateFiles } from '../workspace.mjs';
const [source, destination] = process.argv.slice(2);
if (!source || !destination) throw new Error('Usage: node export-sources.mjs <application-directory> <sources.json>');
const files = await collect(await realpath(source));
validateFiles(files);
await writeFile(destination, JSON.stringify({ files }), { flag: 'wx', mode: 0o600 });
console.log(`Exported ${Object.keys(files).length} source files`);
