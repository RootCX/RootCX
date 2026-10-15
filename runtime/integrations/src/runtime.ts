import Ajv from 'ajv';
import { context, IntegrationError, bytes } from './http';
import { object, string } from './schema';
import type { Credential, Effect, ObjectValue, Plugin } from './types';

const ajv = new Ajv({ strict: true, strictRequired: false, allowUnionTypes: true, allErrors: false, coerceTypes: false, useDefaults: false });
const errorResult = (error: unknown, effect?: Effect) => {
  const e = error instanceof IntegrationError ? error : new IntegrationError('CONNECTOR_ERROR', 'Connector could not complete the operation', undefined, undefined, effect === 'write' ? 'unknown' : 'rejected');
  return { ok: false, error: { code: e.code, message: e.message, status: e.status, retryAfter: e.retryAfter, outcome: e.outcome } };
};

export function createRuntime(plugin: Plugin, fetcher: typeof fetch = fetch) {
  if (plugin.protocol !== 1) throw new Error('Unsupported integration plugin protocol');
  const validators = new Map(Object.entries(plugin.actions).map(([id, action]) => [id, ajv.compile(action.input)]));
  const outputs = new Map(Object.entries(plugin.actions).filter(([, action]) => action.output).map(([id, action]) => [id, ajv.compile(action.output!)]));
  const credentialSchema = ajv.compile(plugin.credentialSchema ?? object({ accessToken: string('Secret', { maxLength: 16384 }), instanceUrl: string() }, ['accessToken']));
  const makeContext = (params: ObjectValue, effect: Effect) => context(plugin, params.userCredentials ?? {}, params.connectionId ?? params.userId ?? 'auth-verification', fetcher, effect);
  const rpc = {
    async __integration(params: ObjectValue) {
      try {
        const validate = validators.get(params.action);
        if (!validate) throw new IntegrationError('UNKNOWN_ACTION', 'Action is not registered');
        const input = params.input ?? {};
        if (!validate(input)) throw new IntegrationError('INVALID_INPUT', `Invalid parameters: ${validate.errors?.[0]?.instancePath || '/'} ${validate.errors?.[0]?.keyword}`);
        const result = await plugin.actions[params.action].run(makeContext(params, plugin.actions[params.action].effect), input as ObjectValue);
        const output = outputs.get(params.action);
        if (output && !output(result)) throw new IntegrationError('INVALID_RESPONSE', 'Provider returned an unexpected response', undefined, undefined, plugin.actions[params.action].effect === 'write' ? 'unknown' : 'rejected');
        return { ok: true, data: result };
      } catch (error) { return errorResult(error, plugin.actions[params.action]?.effect); }
    },
    async __auth_credentials(params: ObjectValue) {
      if (!plugin.tokenAuth) throw new Error('This connector requires OAuth');
      if (!credentialSchema(params.credentials)) throw new Error('Invalid credentials');
      const verified = await plugin.verify(context(plugin, params.credentials as Credential, params.userId, fetcher));
      return { credentials: params.credentials, label: `${verified.label} [${verified.id}]` };
    },
    async __auth_start(params: ObjectValue) {
      const oauth = plugin.oauth;
      if (!oauth) return { type: 'credentials', schema: object({ accessToken: string(plugin.tokenAuth?.label) }, ['accessToken']) };
      if (!params.config?.clientId || !params.config?.clientSecret) throw new Error('Administrator must configure the OAuth application first');
      const url = new URL(oauth.authorizeUrl);
      const values: Record<string, string> = { ...oauth.authorizationParams, client_id: params.config.clientId, redirect_uri: params.callbackUrl, response_type: 'code', state: params.state, scope: oauth.scopes.join(oauth.scopeSeparator ?? ' ') };
      if (oauth.pkce) {
        if (!params.codeChallenge) throw new Error('PKCE challenge is required');
        values.code_challenge = params.codeChallenge; values.code_challenge_method = 'S256';
      }
      for (const [key, value] of Object.entries(values)) url.searchParams.set(key, value);
      return { type: 'redirect', url: url.toString() };
    },
    async __auth_callback(params: ObjectValue) {
      const oauth = plugin.oauth;
      if (!oauth || params.query?.error || !params.query?.code) throw new Error('Authorization was not granted');
      const fields: Record<string, string> = { grant_type: 'authorization_code', code: params.query.code, redirect_uri: params.callbackUrl };
      if (oauth.pkce) {
        if (!params.codeVerifier) throw new Error('PKCE verifier missing');
        fields.code_verifier = params.codeVerifier;
      }
      const headers: Record<string, string> = {};
      if (oauth.clientAuth === 'basic') headers.Authorization = `Basic ${Buffer.from(`${params.config.clientId}:${params.config.clientSecret}`).toString('base64')}`;
      else { fields.client_id = params.config.clientId; fields.client_secret = params.config.clientSecret; }
      headers['Content-Type'] = oauth.tokenEncoding === 'json' ? 'application/json' : 'application/x-www-form-urlencoded';
      const response = await fetcher(oauth.tokenUrl, { method: 'POST', headers, body: oauth.tokenEncoding === 'json' ? JSON.stringify(fields) : new URLSearchParams(fields), redirect: 'error', signal: AbortSignal.timeout(15000) });
      if (!response.ok) { await response.body?.cancel(); throw new Error(`Authorization exchange failed (HTTP ${response.status})`); }
      const token = JSON.parse(new TextDecoder().decode(await bytes(response))) as ObjectValue;
      if (!token || typeof token !== 'object' || typeof token.access_token !== 'string' || !token.access_token.trim() || /[\r\n]/.test(token.access_token) || token.ok === false) throw new Error('Authorization exchange did not issue an access token');
      if (token.refresh_token !== undefined && (typeof token.refresh_token !== 'string' || !token.refresh_token.trim())) throw new Error('Authorization exchange returned an invalid refresh token');
      if (token.expires_in !== undefined && (!Number.isFinite(Number(token.expires_in)) || Number(token.expires_in) <= 0)) throw new Error('Authorization exchange returned an invalid token lifetime');
      const credentials = { accessToken: token.access_token, ...(token.refresh_token ? { refreshToken: token.refresh_token } : {}), ...(token.expires_in ? { _expiresAt: Math.floor(Date.now() / 1000) + Number(token.expires_in) } : {}), ...(token.instance_url ? { instanceUrl: token.instance_url } : {}) };
      const account = await oauth.account(token, context(plugin, credentials, params.userId, fetcher));
      if (typeof account.id !== 'string' || !account.id.trim() || typeof account.label !== 'string' || !account.label.trim()) throw new Error('Authorization account identity missing');
      return { credentials, label: `${account.label} [${account.id}]` };
    },
  };
  return rpc;
}

export function manifest(plugin: Plugin) {
  const prefix = plugin.id.toUpperCase();
  return {
    appId: plugin.id, name: plugin.name, version: plugin.version, type: 'integration', description: plugin.description, entities: [],
    configSchema: plugin.oauth ? object({
      clientId: { ...string('OAuth client ID'), platformSecret: `${prefix}_CLIENT_ID` },
      clientSecret: { ...string('OAuth client secret'), platformSecret: `${prefix}_CLIENT_SECRET` },
      tokenUrl: { ...string(), const: plugin.oauth.tokenUrl, platformSecret: `${prefix}_TOKEN_URL` },
      tokenAuthMethod: { ...string(), const: plugin.oauth.clientAuth ?? 'body', platformSecret: `${prefix}_TOKEN_AUTH_METHOD` },
      tokenEncoding: { ...string(), const: plugin.oauth.tokenEncoding ?? 'form', platformSecret: `${prefix}_TOKEN_ENCODING` },
    }, ['clientId', 'clientSecret', 'tokenUrl']) : object({}),
    userAuth: {
      type: plugin.oauth ? 'oauth2' : 'credentials',
      plugin: { protocolVersion: plugin.protocol, qualification: 'requires-live-verification', discovery: plugin.discovery ?? null },
      tokenAuth: plugin.tokenAuth ?? null,
      oauth: plugin.oauth ? { scopes: plugin.oauth.scopes, tokenUrl: plugin.oauth.tokenUrl, tokenAuthMethod: plugin.oauth.clientAuth ?? 'body', tokenEncoding: plugin.oauth.tokenEncoding ?? 'form' } : null,
      credentialSchema: plugin.credentialSchema ?? object({ accessToken: string(plugin.tokenAuth?.label ?? 'Access token') }, ['accessToken']),
    },
    actions: Object.entries(plugin.actions).map(([id, action]) => ({ id, name: action.name, description: `${action.description} [${action.effect}]`, inputSchema: action.input, outputSchema: action.output ?? {} })),
    instructions: 'RootCX integration plugin protocol 1. Connect an account and bind it to the application. Credentials and permissions belong to Core. Reads are live. Use discovery and schema actions before writing. Follow nextCursor explicitly. Never retry an uncertain write automatically; reconcile with the provider. Only the listed actions are supported. Provider qualification is required before production activation.',
  };
}

export function prefixActions(plugin: Plugin, prefix: string): Plugin {
  return { ...plugin, actions: Object.fromEntries(Object.entries(plugin.actions).map(([id, action]) => [prefix + id, action])), discovery: plugin.discovery ? { ...plugin.discovery, action: prefix + plugin.discovery.action } : undefined };
}
