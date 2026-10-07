import { createServer as httpServer } from 'node:http';
import { createServer as httpsServer } from 'node:https';
import { readFileSync } from 'node:fs';

export function serviceServer(handler, tls = {}) {
  const certFile = tls.certFile ?? process.env.ROOTCX_BUILDER_TLS_CERT_FILE;
  const keyFile = tls.keyFile ?? process.env.ROOTCX_BUILDER_TLS_KEY_FILE;
  if (Boolean(certFile) !== Boolean(keyFile)) throw new Error('Both Builder TLS certificate and key are required');
  if (!certFile) return httpServer(handler);
  const read = () => ({ cert: readFileSync(certFile), key: readFileSync(keyFile), minVersion: 'TLSv1.2' });
  let current = read();
  const server = httpsServer(current, handler);
  // Kubernetes rotates projected Secret volumes; keep the listener and builds alive.
  const rotation = setInterval(() => {
    try {
      const next = read();
      if (next.cert.equals(current.cert) && next.key.equals(current.key)) return;
      server.setSecureContext(next);
      current = next;
    } catch { console.error('Builder TLS certificate reload failed; retaining the last valid certificate'); }
  }, 60_000).unref();
  server.once('close', () => clearInterval(rotation));
  return server;
}
