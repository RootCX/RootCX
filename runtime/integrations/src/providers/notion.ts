import { provider, pick } from './shared';
import { action, array, id, map, pagination, page, segment, string } from '../schema';
import { IntegrationError } from '../http';
import type { Context, ObjectValue } from '../types';

const schema = (ctx: Context, dataSourceId: string) => ctx.request({ path: `/v1/data_sources/${segment(dataSourceId)}` });
const writable = new Set(['title', 'rich_text', 'number', 'select', 'multi_select', 'status', 'date', 'people', 'files', 'checkbox', 'url', 'email', 'phone_number', 'relation']);
async function validateProperties(ctx: Context, dataSourceId: string, values: ObjectValue) {
  const source = await schema(ctx, dataSourceId);
  for (const [key, value] of Object.entries(values)) {
    const field = source.properties?.[key] ?? Object.values(source.properties ?? {}).find((p: any) => p.id === key) as any;
    if (!field || !writable.has(field.type)) throw new IntegrationError('INVALID_INPUT', `Unknown or read-only Notion property: ${key}`);
    if (!value || typeof value !== 'object' || Array.isArray(value) || !Object.hasOwn(value, field.type) || Object.keys(value).some(k => k !== field.type && k !== 'type') || (value.type && value.type !== field.type)) throw new IntegrationError('INVALID_INPUT', `Expected native Notion ${field.type} value for ${key}`);
  }
}
const properties = { ...map, minProperties: 1, description: 'Native Notion property objects keyed by property name or ID; get_schema returns the expected types.' };
const verify = async (ctx: Context) => {
  const user = await ctx.request({ path: '/v1/users/me' });
  if (!user.id) throw new Error('Notion account identifier missing');
  return { id: user.id, label: user.bot?.workspace_name ?? user.name ?? 'Notion' };
};

export default provider('notion', 'Notion', 'https://api.notion.com', {
  list_sources: action('List data sources', 'Discover accessible Notion data sources', 'read', { ...pagination, query: string() }, [], async (ctx, input) => {
    const result = await ctx.request({ path: '/v1/search', method: 'POST', json: { query: input.query, filter: { value: 'data_source', property: 'object' }, page_size: input.limit ?? 50, start_cursor: input.cursor } });
    return page(result.results.map((row: ObjectValue) => ({ ...row, titleText: (row.title ?? []).map((part: any) => part.plain_text ?? part.text?.content ?? '').join('') })), result.next_cursor);
  }),
  get_schema: action('Get source schema', 'Retrieve native Notion property definitions', 'read', { dataSourceId: id() }, ['dataSourceId'], (ctx, input) => schema(ctx, input.dataSourceId)),
  query_entries: action('Query entries', 'Query one page using native Notion filters and sorts', 'read', { dataSourceId: id(), ...pagination, filter: map, sorts: array(map) }, ['dataSourceId'], async (ctx, input) => {
    const result = await ctx.request({ path: `/v1/data_sources/${segment(input.dataSourceId)}/query`, method: 'POST', json: { ...pick(input, ['filter', 'sorts']), page_size: input.limit ?? 50, start_cursor: input.cursor } });
    return page(result.results, result.next_cursor);
  }),
  get_entry: action('Get entry', 'Read a page and its native properties', 'read', { pageId: id() }, ['pageId'], (ctx, input) => ctx.request({ path: `/v1/pages/${segment(input.pageId)}` })),
  create_entry: action('Create entry', 'Create a page in a data source. Never automatically retry an uncertain creation.', 'write', { dataSourceId: id(), properties, children: array(map) }, ['dataSourceId', 'properties'], async (ctx, input) => {
    await validateProperties(ctx, input.dataSourceId, input.properties);
    return ctx.request({ path: '/v1/pages', method: 'POST', json: { parent: { type: 'data_source_id', data_source_id: input.dataSourceId }, ...pick(input, ['properties', 'children']) } });
  }),
  update_entry: action('Update entry', 'Update properties of the existing Notion page', 'write', { pageId: id(), properties }, ['pageId', 'properties'], async (ctx, input) => {
    const entry = await ctx.request({ path: `/v1/pages/${segment(input.pageId)}` });
    if (!entry.parent?.data_source_id) throw new IntegrationError('INVALID_INPUT', 'Page does not belong to a data source');
    await validateProperties(ctx, entry.parent.data_source_id, input.properties);
    return ctx.request({ path: `/v1/pages/${segment(input.pageId)}`, method: 'PATCH', json: { properties: input.properties } });
  }),
}, {
  headers: { 'Notion-Version': '2025-09-03' }, requestsPerSecond: 3, verify,
  tokenAuth: { label: 'Notion internal integration secret', helpUrl: 'https://www.notion.so/profile/integrations' },
  oauth: { authorizeUrl: 'https://api.notion.com/v1/oauth/authorize', tokenUrl: 'https://api.notion.com/v1/oauth/token', scopes: [], clientAuth: 'basic', tokenEncoding: 'json', authorizationParams: { owner: 'user' }, account: async (_token, ctx) => verify(ctx) },
  discovery: { action: 'list_sources', itemsPath: 'items', idPath: 'id', labelPath: 'titleText' },
});
