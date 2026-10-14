import { provider } from './shared';
import { action, id, map, pagination, page, segment, string } from '../schema';
import type { Context } from '../types';

const target = { baseId: id(), tableId: id() };
const fields = { ...map, minProperties: 1 };
const verify = async (ctx: Context) => {
  const user = await ctx.request({ path: '/v0/meta/whoami' });
  if (!user.id) throw new Error('Airtable account identifier missing');
  return { id: user.id, label: user.email ?? user.id };
};
export default provider('airtable', 'Airtable', 'https://api.airtable.com', {
  list_bases: action('List bases', 'Discover accessible bases', 'read', { cursor: pagination.cursor }, [], async (ctx, input) => {
    const result = await ctx.request({ path: '/v0/meta/bases', query: { offset: input.cursor } });
    return page(result.bases, result.offset);
  }),
  get_schema: action('Get base schema', 'Read tables and field definitions for one base', 'read', { baseId: id() }, ['baseId'], (ctx, input) => ctx.request({ path: `/v0/meta/bases/${segment(input.baseId)}/tables` })),
  list_records: action('List records', 'Read one page of table records with an optional native filter formula', 'read', { ...target, ...pagination, filterByFormula: string() }, ['baseId', 'tableId'], async (ctx, input) => {
    const result = await ctx.request({ path: `/v0/${segment(input.baseId)}/${segment(input.tableId)}`, query: { pageSize: input.limit ?? 50, offset: input.cursor, filterByFormula: input.filterByFormula } });
    return page(result.records, result.offset);
  }),
  get_record: action('Get record', 'Read an existing record by its stable ID', 'read', { ...target, recordId: id() }, ['baseId', 'tableId', 'recordId'], (ctx, input) => ctx.request({ path: `/v0/${segment(input.baseId)}/${segment(input.tableId)}/${segment(input.recordId)}` })),
  create_record: action('Create record', 'Create one record using native field values; no automatic retry', 'write', { ...target, fields }, ['baseId', 'tableId', 'fields'], (ctx, input) => ctx.request({ path: `/v0/${segment(input.baseId)}/${segment(input.tableId)}`, method: 'POST', json: { fields: input.fields, typecast: false } })),
  update_record: action('Update record', 'Patch one existing record using native field values', 'write', { ...target, recordId: id(), fields }, ['baseId', 'tableId', 'recordId', 'fields'], (ctx, input) => ctx.request({ path: `/v0/${segment(input.baseId)}/${segment(input.tableId)}/${segment(input.recordId)}`, method: 'PATCH', json: { fields: input.fields, typecast: false } })),
}, { verify, tokenAuth: { label: 'Airtable personal access token', helpUrl: 'https://airtable.com/create/tokens' },
  oauth: { authorizeUrl: 'https://airtable.com/oauth2/v1/authorize', tokenUrl: 'https://airtable.com/oauth2/v1/token', scopes: ['data.records:read', 'data.records:write', 'schema.bases:read'], pkce: true, clientAuth: 'basic', account: async (_token, ctx) => verify(ctx) },
  discovery: { action: 'list_bases', itemsPath: 'items', idPath: 'id', labelPath: 'name' },
});
