import { provider, pick } from './shared';
import { action, id, pagination, page, segment, string } from '../schema';
import type { Context } from '../types';

const metadata = {
  type: 'object', maxProperties: 50,
  propertyNames: { minLength: 1, maxLength: 40, pattern: '^[^\\[\\]]+$' },
  additionalProperties: string('Metadata value; empty string removes this key', { minLength: 0, maxLength: 500 }),
};
const fields = { name: string(), email: string(), description: string(), metadata };
const verify = async (ctx: Context) => {
  const account = await ctx.request({ path: '/v1/account' });
  if (!account.id) throw new Error('Stripe account identifier missing');
  return { id: account.id, label: account.business_profile?.name ?? account.id };
};
export default provider('stripe', 'Stripe', 'https://api.stripe.com', {
  list_customers: action('List customers', 'Read one page of Stripe customers', 'read', { ...pagination, email: string() }, [], async (ctx, input) => {
    const result = await ctx.request({ path: '/v1/customers', query: { limit: input.limit ?? 50, starting_after: input.cursor, email: input.email } });
    return page(result.data, result.has_more ? result.data.at(-1)?.id : null);
  }),
  get_customer: action('Get customer', 'Read one Stripe customer', 'read', { customerId: id() }, ['customerId'], (ctx, input) => ctx.request({ path: `/v1/customers/${segment(input.customerId)}` })),
  create_customer: action('Create customer', 'Create a customer using a stable idempotency key for this intended creation', 'write', { ...fields, idempotencyKey: string('Reuse only for the same intended creation', { maxLength: 255 }) }, ['name', 'idempotencyKey'], (ctx, input) => ctx.request({ path: '/v1/customers', method: 'POST', headers: { 'Idempotency-Key': input.idempotencyKey }, form: pick(input, Object.keys(fields)) })),
  update_customer: action('Update customer', 'Update the existing customer', 'write', { customerId: id(), ...fields }, ['customerId'], (ctx, input) => ctx.request({ path: `/v1/customers/${segment(input.customerId)}`, method: 'POST', form: pick(input, Object.keys(fields)) })),
  list_subscriptions: action('List subscriptions', 'Read customer subscriptions without changing billing', 'read', { ...pagination, customerId: id() }, [], async (ctx, input) => {
    const result = await ctx.request({ path: '/v1/subscriptions', query: { customer: input.customerId, status: 'all', limit: input.limit ?? 50, starting_after: input.cursor } });
    return page(result.data, result.has_more ? result.data.at(-1)?.id : null);
  }),
}, { headers: { 'Stripe-Version': '2025-09-30.clover' }, verify, tokenAuth: { label: 'Stripe secret or restricted API key (account read and selected customer/subscription permissions)', helpUrl: 'https://dashboard.stripe.com/apikeys' } });
