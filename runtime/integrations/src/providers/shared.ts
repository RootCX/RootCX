import type { Context, OAuth, Plugin, ObjectValue, Effect } from '../types';
import { IntegrationError } from '../http';
import { action } from '../schema';

export function provider(id: string, name: string, origin: Plugin['origin'], actions: Plugin['actions'], rest: Partial<Plugin> = {}): Plugin {
  const verify = rest.verify ?? (rest.oauth ? (ctx: Context) => rest.oauth!.account({}, ctx) : async () => { throw new Error('Account verification is required'); });
  return { protocol: 1, id, name, version: '1.1.0', description: `Connect ${name} and operate on live data.`, origin, requestsPerSecond: 5, ...rest, verify, actions: { verify_connection: action('Verify connection', 'Verify the connected account with the provider', 'read', {}, [], async ctx => verify(ctx)), ...actions } };
}
export const googleOAuth = (scopes: string[]): OAuth => ({
  authorizeUrl: 'https://accounts.google.com/o/oauth2/v2/auth', tokenUrl: 'https://oauth2.googleapis.com/token',
  scopes: ['openid', 'email', ...scopes], pkce: true,
  authorizationParams: { access_type: 'offline', prompt: 'consent' },
  async account(_token, ctx) {
    const user = await ctx.request({ service: 'identity', path: '/v1/userinfo' });
    if (!user.sub || !user.email) throw new Error('Google account identifier missing');
    return { id: user.sub, label: user.email };
  },
});
export const googleServices = { identity: 'https://openidconnect.googleapis.com' };
export function pick(value: ObjectValue, keys: string[]) { return Object.fromEntries(keys.filter(key => value[key] !== undefined).map(key => [key, value[key]])); }
export async function slackRequest(ctx: Context, path: string, body?: ObjectValue, effect: Effect = 'read') {
  const result = await ctx.request({ path: `/api/${path}`, ...(body ? { method: 'POST' as const, json: body } : {}) });
  if (!result || result.ok !== true) {
    const error = typeof result?.error === 'string' && /^[a-z_]+$/.test(result.error) ? result.error : 'unknown_error';
    const uncertain = effect === 'write' && ['internal_error', 'fatal_error', 'unknown_error'].includes(error);
    const code = ['invalid_auth', 'token_revoked', 'token_expired'].includes(error) ? 'unauthorized' : 'PROVIDER_REJECTED';
    throw new IntegrationError(uncertain ? 'WRITE_UNCERTAIN' : code, `Slack could not complete the operation (${error})`, undefined, undefined, uncertain ? 'unknown' : 'rejected');
  }
  return result;
}
