import { lstat, mkdir, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import { join } from 'node:path';

const forbidden = new Set(['.', '..', '.git', '.gitattributes', '.aws', '.ssh', '.kube', '.rootcx', 'node_modules', 'dist', '.npmrc', '.yarnrc', '.yarnrc.yml']);
export function sourcePath(name) {
  if (typeof name !== 'string' || !name || name.length > 500 || /[\\\0:]/.test(name)
      || name.split('/').some(part => !part || forbidden.has(part) || (part.startsWith('.env') && part !== '.env.example'))) {
    throw new Error('Invalid source path');
  }
  return name;
}

export function validateFiles(files) {
  if (!files || typeof files !== 'object' || Array.isArray(files)) throw new Error('Sources required');
  const entries = Object.entries(files);
  if (!entries.length || entries.length > 4000) throw new Error('Source file limit exceeded');
  let bytes = 0;
  for (const [name, content] of entries) {
    sourcePath(name);
    if (typeof content !== 'string' || content.length > 90 * 1024 * 1024) throw new Error('Invalid source encoding');
    const decoded = Buffer.from(content, 'base64');
    if (decoded.toString('base64') !== content) throw new Error('Invalid source encoding');
    bytes += decoded.length;
    if (bytes > 64 * 1024 * 1024) throw new Error('Source size limit exceeded');
  }
}

export async function materialize(root, files) {
  validateFiles(files);
  for (const [name, content] of Object.entries(files)) {
    const path = join(root, name);
    await mkdir(join(path, '..'), { recursive: true });
    await writeFile(path, Buffer.from(content, 'base64'), { flag: 'wx', mode: 0o600 });
  }
}

// Agent tools cannot follow build-created symlinks, including parent directories.
export async function confined(root, name) {
  sourcePath(name);
  let path = root;
  for (const part of name.split('/')) {
    path = join(path, part);
    try {
      const stat = await lstat(path);
      if (stat.isSymbolicLink() || (!stat.isFile() && !stat.isDirectory())) throw new Error('Links and special files are forbidden');
    } catch (error) { if (error.code !== 'ENOENT') throw error; }
  }
  return path;
}

export async function collect(root, prefix = '', budget = { bytes: 0, files: 0 }) {
  const files = {};
  for (const entry of await readdir(join(root, prefix), { withFileTypes: true })) {
    if (forbidden.has(entry.name)) continue;
    const name = prefix ? `${prefix}/${entry.name}` : entry.name;
    sourcePath(name);
    if (entry.isSymbolicLink() || (!entry.isDirectory() && !entry.isFile())) throw new Error('Links and special files are forbidden');
    if (entry.isDirectory()) Object.assign(files, await collect(root, name, budget));
    else {
      budget.files++;
      budget.bytes += (await lstat(join(root, name))).size;
      if (budget.files > 4000 || budget.bytes > 64 * 1024 * 1024) throw new Error('Source limit exceeded');
      files[name] = (await readFile(join(root, name))).toString('base64');
    }
  }
  return files;
}

export async function edit(root, name, content) {
  const path = await confined(root, name);
  if (typeof content !== 'string' || Buffer.byteLength(content) > 2 * 1024 * 1024) throw new Error('File edit limit exceeded');
  await mkdir(join(path, '..'), { recursive: true });
  await writeFile(path, content, { mode: 0o600 });
}

export async function remove(root, name) {
  const path = await confined(root, name);
  const stat = await lstat(path);
  if (!stat.isFile()) throw new Error('Only individual source files can be removed');
  await rm(path);
}
