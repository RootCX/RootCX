import { createServer } from 'node:http';
import { Readable } from 'node:stream';
import { pipeline } from 'node:stream/promises';

export function createProviderProxy(config, proxyKey) {
  return createServer(async (req, res) => {
    const [path, query] = (req.url ?? '').split('?');
    // The native SDK uses Anthropic's beta endpoint; the upstream is still fixed
    // by Core configuration, and model requests cannot choose another route.
    const supported = ['/v1/messages', '/v1/messages/count_tokens'].includes(path)
      && (query === undefined || query === 'beta=true') && req.url.split('?').length <= 2;
    if (req.headers['x-api-key'] !== proxyKey || req.method !== 'POST' || !supported) {
      console.error(JSON.stringify({ event: 'provider.request_rejected', reason: req.headers['x-api-key'] !== proxyKey ? 'authentication' : req.method !== 'POST' ? 'method' : 'path', path: req.url?.split('?')[0] }));
      res.writeHead(403); res.end(); return;
    }
    const controller = new AbortController();
    res.on('close', () => { if (!res.writableEnded) controller.abort(); });
    try {
      const upstream = new URL(config.endpoint);
      if (path.endsWith('/count_tokens')) upstream.pathname += '/count_tokens';
      const headers = { 'content-type': 'application/json', 'x-api-key': config.apiKey, 'anthropic-version': '2023-06-01' };
      if (req.headers['anthropic-beta']) headers['anthropic-beta'] = req.headers['anthropic-beta'];
      const response = await fetch(upstream, { method: 'POST', headers, body: req, duplex: 'half', redirect: 'error', signal: controller.signal });
      res.writeHead(response.status, { 'content-type': response.headers.get('content-type') || 'application/json' });
      if (response.body) await pipeline(Readable.fromWeb(response.body), res); else res.end();
    } catch (error) {
      if (controller.signal.aborted) return;
      const cause = error.cause?.code ?? error.code;
      const disconnected = ['ECONNREFUSED', 'ENOTFOUND', 'EAI_NONAME'].includes(cause);
      console.error(JSON.stringify({ event: 'provider.transport_failed', cause: cause ?? 'UNKNOWN', disconnected }));
      if (res.headersSent) { res.destroy(); return; }
      // Internal adapter: a missing endpoint cannot recover through model retries.
      // Keep real upstream 429/5xx responses retryable by the native engine.
      res.writeHead(disconnected ? 400 : 502, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ type: 'error', error: {
        type: disconnected ? 'invalid_request_error' : 'api_error',
        code: disconnected ? 'AI_CONFIGURATION' : 'AI_UNAVAILABLE',
        message: disconnected ? 'The configured AI connection is unavailable.' : 'The AI connection was interrupted.',
      } }));
    }
  });
}
