import { provider } from './shared';
import { action, array, id, map, pagination, page, segment, string } from '../schema';
import { IntegrationError } from '../http';
import type { Context } from '../types';

const contactsPath = (ctx: Context) => ctx.hubspotUserLevel ? '/crm/objects/2026-09/contacts' : '/crm/v3/objects/contacts';
const verify = async (ctx: Context) => {
  if (ctx.hubspotUserLevel) {
    if (!ctx.hubspotUserIdentity) throw new IntegrationError('unauthorized', 'Reconnect the HubSpot user account');
    await ctx.request({ path: contactsPath(ctx), query: { limit: 1, properties: 'hs_object_id' } });
    return ctx.hubspotUserIdentity;
  }
  const account = await ctx.request({ path: '/account-info/v3/details' });
  if (!Number.isSafeInteger(account.portalId) || account.portalId <= 0) throw new Error('HubSpot account identifier missing');
  return { id: String(account.portalId), label: `HubSpot ${account.portalId}` };
};
const properties = { ...map, minProperties: 1, additionalProperties: { type: ['string', 'number', 'boolean', 'null'] } };
export default provider('hubspot', 'HubSpot', 'https://api.hubapi.com', {
  list_contacts: action('List contacts', 'Read one page of contacts', 'read', { ...pagination, properties: array(string()) }, [], async (ctx, input) => {
    const result = await ctx.request({ path: contactsPath(ctx), query: { limit: input.limit ?? 50, after: input.cursor, properties: input.properties?.join(',') } });
    return page(result.results, result.paging?.next?.after?.toString());
  }),
  search_contacts: action('Search contacts', 'Search contact values and native filter groups', 'read', { ...pagination, query: string(), filterGroups: array(map, 5), properties: array(string()) }, [], async (ctx, input) => {
    if (input.cursor !== undefined && (!/^(0|[1-9][0-9]{0,3})$/.test(input.cursor))) throw new IntegrationError('INVALID_INPUT', 'Search cursor must be an integer below 10000; narrow the query to retrieve further results');
    const result = await ctx.request({ path: `${contactsPath(ctx)}/search`, method: 'POST', json: { query: input.query, filterGroups: input.filterGroups, properties: input.properties, limit: input.limit ?? 50, after: input.cursor === undefined ? undefined : Number(input.cursor) } });
    return page(result.results, result.paging?.next?.after?.toString());
  }),
  get_schema: action('Get contact schema', 'Discover writable contact property definitions', 'read', {}, [], ctx => {
    if (ctx.hubspotUserLevel) throw new IntegrationError('UNSUPPORTED_OPERATION', 'HubSpot does not support contact schema discovery for user-level connections; supply known property names from your HubSpot account');
    return ctx.request({ path: '/crm/v3/properties/contacts' });
  }),
  get_contact: action('Get contact', 'Read an existing contact', 'read', { contactId: id(), properties: array(string()) }, ['contactId'], (ctx, input) => ctx.request({ path: `${contactsPath(ctx)}/${segment(input.contactId)}`, query: { properties: input.properties?.join(',') } })),
  create_contact: action('Create contact', 'Create a contact; reconcile an uncertain write before retrying', 'write', { properties }, ['properties'], (ctx, input) => ctx.request({ path: contactsPath(ctx), method: 'POST', json: ctx.hubspotUserLevel ? { ...input, associations: [] } : input })),
  update_contact: action('Update contact', 'Update the existing contact by its ID', 'write', { contactId: id(), properties }, ['contactId', 'properties'], (ctx, input) => ctx.request({ path: `${contactsPath(ctx)}/${segment(input.contactId)}`, method: 'PATCH', json: { properties: input.properties } })),
}, { version: '1.2.1', verify, tokenAuth: { label: 'HubSpot private app token', helpUrl: 'https://developers.hubspot.com/docs/apps/legacy-apps/private-apps/overview' },
  oauth: { authorizeUrl: 'https://app.hubspot.com/oauth/authorize', tokenUrl: 'https://api.hubspot.com/oauth/v3/token', scopes: ['oauth', 'crm.objects.contacts.read', 'crm.objects.contacts.write', 'crm.schemas.contacts.read'], account: async (_token, ctx) => verify(ctx) },
});
