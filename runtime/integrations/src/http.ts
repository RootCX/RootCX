import type { Context, Credential, Effect, Plugin, Request } from './types';

export class IntegrationError extends Error {
  constructor(public code: string, message: string, public status?: number, public retryAfter?: number, public outcome: 'rejected' | 'unknown' = 'rejected') { super(message); }
}
const MAX_BYTES = 10 * 1024 * 1024;
const COOLDOWN = new Map<string, number>();
const RATE = new Map<string, { tokens: number; at: number }>();
const IN_FLIGHT = new Map<string, number>();

export async function bytes(response: Response): Promise<Uint8Array> {
  if (Number(response.headers.get('content-length')) > MAX_BYTES) {
    await response.body?.cancel().catch(() => {});
    throw new IntegrationError('RESPONSE_TOO_LARGE', 'Response exceeds 10 MiB');
  }
  const reader = response.body?.getReader();
  if (!reader) return new Uint8Array();
  const chunks: Uint8Array[] = []; let length = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      length += value.length;
      if (length > MAX_BYTES) throw new IntegrationError('RESPONSE_TOO_LARGE', 'Response exceeds 10 MiB');
      chunks.push(value);
    }
  } finally { await reader.cancel().catch(() => {}); }
  const output = new Uint8Array(length); let offset = 0;
  for (const chunk of chunks) { output.set(chunk, offset); offset += chunk.length; }
  return output;
}

export function formBody(values: Record<string, unknown>): URLSearchParams {
  const output = new URLSearchParams();
  const add = (key: string, value: unknown) => {
    if (value === undefined) return;
    if (value !== null && typeof value === 'object') {
      for (const [child, item] of Object.entries(value)) add(`${key}[${child}]`, item);
    } else output.append(key, value === null ? '' : String(value));
  };
  for (const [key, value] of Object.entries(values)) add(key, value);
  return output;
}

export function context(plugin: Plugin, credential: Credential, connection: string, fetcher: typeof fetch = fetch, effect?: Effect): Context {
  if (!credential.accessToken || /[\r\n]/.test(credential.accessToken)) throw new IntegrationError('unauthorized', 'Connect an account first');
  const origin = new URL(typeof plugin.origin === 'function' ? plugin.origin(credential) : plugin.origin);
  if (origin.protocol !== 'https:' || origin.username || origin.password || origin.pathname !== '/' || origin.search || origin.hash) throw new IntegrationError('CONFIGURATION_ERROR', 'Invalid provider origin');
  const key = `${plugin.id}:${connection}`;
  return { async request<T>(request: Request): Promise<T> {
    const method = request.method ?? 'GET';
    const write = effect ? effect === 'write' && method !== 'GET' : method !== 'GET';
    const base = request.service ? plugin.services?.[request.service] : origin.origin;
    if (!base || new URL(base).protocol !== 'https:') throw new IntegrationError('INVALID_INPUT', 'Unknown provider service');
    const url = new URL(request.path, base);
    if (!request.path.startsWith('/') || request.path.startsWith('//') || url.origin !== new URL(base).origin || url.hash || request.path.includes('\\')) throw new IntegrationError('INVALID_INPUT', 'Invalid provider path');
    for (const [name, value] of Object.entries(request.query ?? {})) {
      if (value !== undefined && value !== null) {
        for (const v of Array.isArray(value) ? value : [value]) url.searchParams.append(name, String(v));
      }
    }
    const now = Date.now();
    if ((COOLDOWN.get(key) ?? 0) > now) throw new IntegrationError('RATE_LIMITED', 'Provider rate limit; retry after the indicated delay', 429, Math.ceil((COOLDOWN.get(key)! - now) / 1000));
    if ((IN_FLIGHT.get(key) ?? 0) >= 4) throw new IntegrationError('RATE_LIMITED', 'Too many concurrent requests for this connection', 429, 1);
    // Bound memory in long-lived workers. No credentials are used as map keys.
    if (COOLDOWN.size > 10000) for (const [k, until] of COOLDOWN) if (until <= now) COOLDOWN.delete(k);
    const headers = new Headers(plugin.headers);
    for (const [name, value] of Object.entries(request.headers ?? {})) {
      if (['authorization', 'host', 'cookie'].includes(name.toLowerCase())) throw new IntegrationError('INVALID_INPUT', 'Reserved request header');
      headers.set(name, value);
    }
    let body = request.body;
    if (request.json !== undefined) { headers.set('Content-Type', 'application/json'); body = JSON.stringify(request.json); }
    if (request.form) { headers.set('Content-Type', 'application/x-www-form-urlencoded'); body = formBody(request.form); }
    if (body !== undefined && (body instanceof Blob ? body.size : Buffer.byteLength(String(body))) > MAX_BYTES) throw new IntegrationError('INVALID_INPUT', 'Request exceeds 10 MiB');
    const authorization = plugin.authorize ? await plugin.authorize(credential, { url: new URL(url), method, body }) : { Authorization: `Bearer ${credential.accessToken}` };
    for (const [name, value] of Object.entries(authorization)) headers.set(name, value);
    const rate = Math.max(1, plugin.requestsPerSecond ?? 5);
    const bucket = RATE.get(key) ?? { tokens: rate, at: now };
    bucket.tokens = Math.min(rate, bucket.tokens + (now - bucket.at) / 1000 * rate); bucket.at = now;
    if (bucket.tokens < 1) throw new IntegrationError('RATE_LIMITED', 'Connection request budget exhausted', 429, 1);
    bucket.tokens -= 1; RATE.set(key, bucket);
    if (RATE.size > 10000) for (const [k, b] of RATE) if (now - b.at > 60000) RATE.delete(k);
    IN_FLIGHT.set(key, (IN_FLIGHT.get(key) ?? 0) + 1);
    let response: Response | undefined;
    try {
      response = await fetcher(url, { method, headers, body, redirect: 'error', signal: AbortSignal.timeout(20000) });
      if (!response.ok) {
        await response.body?.cancel().catch(() => {});
        const retry = response.headers.get('retry-after');
        const seconds = retry && /^\d+$/.test(retry) ? Number(retry) : retry ? Math.ceil((Date.parse(retry) - Date.now()) / 1000) : NaN;
        const delay = Number.isFinite(seconds) ? Math.min(86400, Math.max(1, seconds)) : 60;
        if (response.status === 429) COOLDOWN.set(key, Date.now() + delay * 1000);
        const code = response.status === 401 ? 'unauthorized' : response.status === 403 ? 'FORBIDDEN' : response.status === 429 ? 'RATE_LIMITED' : response.status >= 500 ? 'PROVIDER_UNAVAILABLE' : 'PROVIDER_REJECTED';
        throw new IntegrationError(code, `${plugin.name} returned HTTP ${response.status}`, response.status, response.status === 429 ? delay : undefined, write && response.status >= 500 ? 'unknown' : 'rejected');
      }
      const content = await bytes(response);
      if (request.response === 'binary') return { contentBase64: Buffer.from(content).toString('base64'), contentType: response.headers.get('content-type'), size: content.length } as T;
      if (!content.length) return {} as T;
      return JSON.parse(new TextDecoder().decode(content)) as T;
    } catch (error) {
      if (error instanceof IntegrationError) {
        if (write && response?.ok) error.outcome = 'unknown';
        throw error;
      }
      throw new IntegrationError('TRANSPORT_ERROR', write ? 'Write result is uncertain; reconcile with the provider before retrying' : 'Provider response unavailable', undefined, undefined, write ? 'unknown' : 'rejected');
    } finally {
      const remaining = (IN_FLIGHT.get(key) ?? 1) - 1;
      if (remaining) IN_FLIGHT.set(key, remaining); else IN_FLIGHT.delete(key);
    }
  } };
}
