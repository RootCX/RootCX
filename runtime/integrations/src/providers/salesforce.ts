import { provider } from './shared';
import { action, id, map, pagination, page, segment, string } from '../schema';
import { IntegrationError } from '../http';
import type { Context, Credential } from '../types';

const API = '/services/data/v61.0';
const objectType = string('Supported Salesforce object', { enum: ['Contact', 'Opportunity'] });
export function salesforceOrigin(credential: Credential): string {
  const url = new URL(credential.instanceUrl ?? 'https://invalid.example');
  if (url.protocol !== 'https:' || url.port || url.username || url.password || url.pathname !== '/' || url.search || url.hash || !/^[a-z0-9][a-z0-9.-]*\.(?:my\.)?salesforce\.com$/.test(url.hostname)) throw new IntegrationError('CONFIGURATION_ERROR', 'Invalid Salesforce instance URL');
  return url.origin;
}
const verify = async (ctx: Context) => {
  const account = await ctx.request({ path: '/services/oauth2/userinfo' });
  if (typeof account.organization_id !== 'string' || !account.organization_id || typeof account.user_id !== 'string' || !account.user_id) throw new Error('Salesforce account identifier missing');
  return { id: `${account.organization_id}:${account.user_id}`, label: account.preferred_username ?? account.user_id };
};
export default provider('salesforce', 'Salesforce', salesforceOrigin, {
  get_schema: action('Get object schema', 'Discover fields, types and writable properties', 'read', { objectType }, ['objectType'], (ctx, input) => ctx.request({ path: `${API}/sobjects/${input.objectType}/describe` })),
  query_records: action('Query records', 'Execute read-only SOQL, or follow the previous nextCursor. Queries must target Contact or Opportunity.', 'read', { query: string('SOQL SELECT query', { maxLength: 10000 }), cursor: pagination.cursor }, [], async (ctx, input) => {
    const statement = (input.query ?? '').replace(/'(?:\\.|[^'\\])*'/g, "''");
    if (input.cursor) {
      if (input.query || !/^\/services\/data\/v61\.0\/query\/[A-Za-z0-9_-]+$/.test(input.cursor)) throw new IntegrationError('INVALID_INPUT', 'Invalid Salesforce query cursor');
    } else if (!/^\s*SELECT\s+[\s\S]+\s+FROM\s+(Contact|Opportunity)\b/i.test(statement) || /\bFROM\s+(?!Contact\b|Opportunity\b)[A-Za-z_]/i.test(statement)) throw new IntegrationError('INVALID_INPUT', 'Expected a SELECT query on Contact or Opportunity');
    // SOQL can mutate usage metadata or acquire locks despite using the query endpoint.
    if (/\b(?:FOR\s+(?:UPDATE|VIEW|REFERENCE)|UPDATE\s+(?:TRACKING|VIEWSTAT))\b/i.test(statement)) throw new IntegrationError('INVALID_INPUT', 'Only read-only SOQL is supported');
    const result = await ctx.request({ path: input.cursor ?? `${API}/query`, query: input.cursor ? {} : { q: input.query } });
    if (typeof result.done !== 'boolean' || (!result.done && (typeof result.nextRecordsUrl !== 'string' || !/^\/services\/data\/v61\.0\/query\/[A-Za-z0-9_-]+$/.test(result.nextRecordsUrl)))) throw new IntegrationError('INVALID_RESPONSE', 'Salesforce returned an invalid continuation');
    return page(result.records, result.done ? null : result.nextRecordsUrl);
  }),
  get_record: action('Get record', 'Read a contact or opportunity', 'read', { objectType, recordId: id() }, ['objectType', 'recordId'], (ctx, input) => ctx.request({ path: `${API}/sobjects/${input.objectType}/${segment(input.recordId)}` })),
  create_record: action('Create record', 'Create a contact or opportunity with native fields; no automatic retry', 'write', { objectType, fields: { ...map, minProperties: 1 } }, ['objectType', 'fields'], (ctx, input) => ctx.request({ path: `${API}/sobjects/${input.objectType}`, method: 'POST', json: input.fields })),
  update_record: action('Update record', 'Patch the existing contact or opportunity', 'write', { objectType, recordId: id(), fields: { ...map, minProperties: 1 } }, ['objectType', 'recordId', 'fields'], async (ctx, input) => {
    await ctx.request({ path: `${API}/sobjects/${input.objectType}/${segment(input.recordId)}`, method: 'PATCH', json: input.fields });
    return { id: input.recordId, updated: true };
  }),
}, { version: '1.1.1', verify, oauth: { authorizeUrl: 'https://login.salesforce.com/services/oauth2/authorize', tokenUrl: 'https://login.salesforce.com/services/oauth2/token', scopes: ['api', 'refresh_token', 'openid'], pkce: true, account: async (_token, ctx) => verify(ctx) } });
