import { provider } from './shared';
import { action, array, id, map, pagination, page, segment, string } from '../schema';
import type { Context } from '../types';

const verify = async (ctx: Context) => {
  const account = await ctx.request({ path: '/account-info/v3/details' });
  if (!account.portalId) throw new Error('HubSpot account identifier missing');
  return { id: String(account.portalId), label: `HubSpot ${account.portalId}` };
};
const properties = { ...map, minProperties: 1, additionalProperties: { type: ['string', 'number', 'boolean', 'null'] } };
export default provider('hubspot', 'HubSpot', 'https://api.hubapi.com', {
  list_contacts: action('List contacts', 'Read one page of contacts', 'read', { ...pagination, properties: array(string()) }, [], async (ctx, input) => {
    const result = await ctx.request({ path: '/crm/v3/objects/contacts', query: { limit: input.limit ?? 50, after: input.cursor, properties: input.properties?.join(',') } });
    return page(result.results, result.paging?.next?.after?.toString());
  }),
  search_contacts: action('Search contacts', 'Search contact values and native filter groups', 'read', { ...pagination, query: string(), filterGroups: array(map, 5), properties: array(string()) }, [], async (ctx, input) => {
    const result = await ctx.request({ path: '/crm/v3/objects/contacts/search', method: 'POST', json: { query: input.query, filterGroups: input.filterGroups, properties: input.properties, limit: input.limit ?? 50, after: input.cursor } });
    return page(result.results, result.paging?.next?.after?.toString());
  }),
  get_schema: action('Get contact schema', 'Discover writable contact property definitions', 'read', {}, [], ctx => ctx.request({ path: '/crm/v3/properties/contacts' })),
  get_contact: action('Get contact', 'Read an existing contact', 'read', { contactId: id(), properties: array(string()) }, ['contactId'], (ctx, input) => ctx.request({ path: `/crm/v3/objects/contacts/${segment(input.contactId)}`, query: { properties: input.properties?.join(',') } })),
  create_contact: action('Create contact', 'Create a contact; reconcile an uncertain write before retrying', 'write', { properties }, ['properties'], (ctx, input) => ctx.request({ path: '/crm/v3/objects/contacts', method: 'POST', json: input })),
  update_contact: action('Update contact', 'Update the existing contact by its ID', 'write', { contactId: id(), properties }, ['contactId', 'properties'], (ctx, input) => ctx.request({ path: `/crm/v3/objects/contacts/${segment(input.contactId)}`, method: 'PATCH', json: { properties: input.properties } })),
}, { verify, tokenAuth: { label: 'HubSpot private app token', helpUrl: 'https://developers.hubspot.com/docs/apps/legacy-apps/private-apps/overview' },
  oauth: { authorizeUrl: 'https://app.hubspot.com/oauth/authorize', tokenUrl: 'https://api.hubapi.com/oauth/v1/token', scopes: ['oauth', 'crm.objects.contacts.read', 'crm.objects.contacts.write', 'crm.schemas.contacts.read'], account: async (_token, ctx) => verify(ctx) },
});
