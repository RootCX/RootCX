// Deploy separately from Core, behind TLS, with independently backed persistent storage.
import { serviceServer } from './service-server.mjs';
import { createHash, randomUUID, timingSafeEqual } from 'node:crypto';
import { open, mkdir, link, unlink, readFile } from 'node:fs/promises';
import { join } from 'node:path';

export function backupReceiver({ token, directory, tls }) {
  if (!token || token.length < 32 || !directory) throw new Error('Backup receiver configuration incomplete');
  return serviceServer(async (req, res) => {
    const reply = (status, body) => { res.writeHead(status, { 'content-type': 'application/json' }); res.end(JSON.stringify(body)); };
    if (req.method === 'GET' && req.url === '/health') return reply(200, { ready: true });
    const actual = Buffer.from(req.headers.authorization ?? '');
    const expected = Buffer.from(`Bearer ${token}`);
    if (actual.length !== expected.length || !timingSafeEqual(actual, expected)) return reply(401, { error: 'Unauthorized' });
    const match = req.url?.match(/^\/([a-z][a-z0-9_]{0,49})\/([a-f0-9]{40})\.bundle$/);
    if (req.method !== 'PUT' || !match) return reply(400, { error: 'Invalid backup object' });
    const appDirectory = join(directory, match[1]);
    const staging = join(appDirectory, `.${randomUUID()}.tmp`);
    const destination = join(appDirectory, `${match[2]}.bundle`);
    let file;
    try {
      await mkdir(appDirectory, { recursive: true, mode: 0o700 });
      file = await open(staging, 'wx', 0o600);
      let size = 0; const digest = createHash('sha256'); let header = Buffer.alloc(0);
      for await (const chunk of req) {
        size += chunk.length;
        if (size > 512 * 1024 * 1024) throw new Error('Backup exceeds limit');
        if (header.length < 16) header = Buffer.concat([header, chunk.subarray(0, 16 - header.length)]);
        digest.update(chunk);
        // FileHandle.write may legally write fewer bytes than requested.
        let offset = 0;
        while (offset < chunk.length) offset += (await file.write(chunk, offset, chunk.length - offset)).bytesWritten;
      }
      if (!/^# v[23] git bundle/.test(header.toString())) throw new Error('Invalid Git bundle');
      await file.sync(); await file.close(); file = null;
      const sha256 = digest.digest('hex');
      try { await link(staging, destination); }
      catch (error) {
        if (error.code !== 'EEXIST') throw error;
        const existing = createHash('sha256').update(await readFile(destination)).digest('hex');
        if (existing !== sha256) return reply(409, { error: 'Backup object already exists with different contents' });
      }
      // Persist the directory entry before acknowledging durability to Core.
      const dir = await open(appDirectory, 'r');
      try { await dir.sync(); } finally { await dir.close(); }
      const parent = await open(directory, 'r');
      try { await parent.sync(); } finally { await parent.close(); }
      reply(201, { sha256, bytes: size });
    } catch (error) {
      console.error('Source backup failed:', error.message);
      if (!res.headersSent) reply(500, { error: 'Backup not persisted' });
    } finally {
      await file?.close();
      await unlink(staging).catch(() => {});
    }
  }, tls);
}

if (import.meta.url === new URL(process.argv[1], 'file:').href) {
  const server = backupReceiver({ token: process.env.ROOTCX_BUILDER_BACKUP_TOKEN, directory: process.env.ROOTCX_BACKUP_DIRECTORY });
  server.requestTimeout = 120_000;
  server.headersTimeout = 15_000;
  server.listen(Number(process.env.PORT ?? 9202), process.env.BIND ?? '0.0.0.0');
}
