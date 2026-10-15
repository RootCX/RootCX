import { provider, pick } from './shared';
import { action, id, pagination, page, segment, string, map } from '../schema';
import { formBody, IntegrationError } from '../http';
import type { Context, ObjectValue, Request } from '../types';

const idempotencyKey = string('Stable key for one intended operation; reuse with identical parameters only. Reconcile uncertain results before retrying.', { maxLength: 255, pattern: '^[!-~]+$' });
const parameters = { ...map, description: 'Stripe API parameters as nested JSON (arrays supported); use provider IDs, never raw card data or credentials.' };
const apiPath = string('Stripe resource path, without host, query or fragment', { maxLength: 1024, pattern: '^/v[12]/[A-Za-z0-9_-]+(?:/[A-Za-z0-9_-]+)*$' });

// Reject ambiguous form keys before serialization, including prototype and bracket injection.
function validateParameters(value: unknown, depth = 0): void {
  if (depth > 16) throw new IntegrationError('INVALID_INPUT', 'Stripe parameters are nested too deeply');
  if (!value || typeof value !== 'object') return;
  for (const [key, child] of Object.entries(value)) {
    if (!key || /[\[\]\x00-\x1f]/.test(key) || ['__proto__', 'constructor', 'prototype'].includes(key)) throw new IntegrationError('INVALID_INPUT', 'Invalid Stripe parameter key');
    validateParameters(child, depth + 1);
  }
}
// Stripe v1 clears nested values with an empty form value; do not silently omit empty arrays/maps.
function formValues(value: any): any {
  if (!value || typeof value !== 'object') return value;
  if (!Object.keys(value).length) return '';
  return Array.isArray(value) ? value.map(formValues) : Object.fromEntries(Object.entries(value).map(([key, child]) => [key, formValues(child)]));
}
function redact(value: any): any {
  if (Array.isArray(value)) return value.map(redact);
  if (value && typeof value === 'object') return Object.fromEntries(Object.entries(value).map(([key, child]) => [key, /^(?:client_secret|secret|access_token|refresh_token|api_key)$/.test(key) ? '[REDACTED]' : redact(child)]));
  return typeof value === 'string' ? value.replace(/\b(?:[sr]k_(?:live|test)_[A-Za-z0-9]+|whsec_[A-Za-z0-9]+|[A-Za-z0-9_]+_secret_[A-Za-z0-9]+)\b/g, '[REDACTED]') : value;
}
async function request(ctx: Context, options: Request) { return redact(await ctx.request(options)); }
async function api(ctx: Context, path: string, method: 'GET' | 'POST' | 'DELETE', values: ObjectValue = {}, key?: string) {
  validateParameters(values);
  return request(ctx, { path, method,
    ...(key ? { headers: { 'Idempotency-Key': key } } : {}),
    ...(method === 'GET' || (method === 'DELETE' && !path.startsWith('/v2/')) ? { query: Object.fromEntries(formBody(values)) } : path.startsWith('/v2/') ? { json: values } : { form: Object.fromEntries(Object.entries(values).map(([key, value]) => [key, formValues(value)])) }),
  });
}
const invoiceWrites = Object.fromEntries([
  ['update_invoice', '', 'Update an invoice; Stripe restricts fields by invoice state'],
  ['finalize_invoice', '/finalize', 'Finalize a draft invoice; this can trigger automatic collection'],
  ['send_invoice', '/send', 'Send the invoice email to the customer; requires explicit authorization to communicate'],
  ['pay_invoice', '/pay', 'Attempt payment of an invoice; may charge the customer'],
  ['void_invoice', '/void', 'Permanently void a finalized invoice'],
  ['mark_invoice_uncollectible', '/mark_uncollectible', 'Mark an invoice as uncollectible'],
].map(([name, suffix, description]) => [name, action(name.replaceAll('_', ' '), description, 'write', { invoiceId: id(), parameters, idempotencyKey }, ['invoiceId', 'idempotencyKey'], (ctx, input) => api(ctx, `/v1/invoices/${segment(input.invoiceId)}${suffix}`, 'POST', input.parameters, input.idempotencyKey))]));

const metadata = {
  type: 'object', maxProperties: 50,
  propertyNames: { minLength: 1, maxLength: 40, pattern: '^[^\\[\\]]+$' },
  additionalProperties: string('Metadata value; empty string removes this key', { minLength: 0, maxLength: 500 }),
};
const fields = { name: string(), email: string(), description: string(), metadata };
const verify = async (ctx: Context) => {
  const account = await request(ctx, { path: '/v1/account' });
  if (!account.id) throw new Error('Stripe account identifier missing');
  return { id: account.id, label: account.business_profile?.name ?? account.id };
};
export default provider('stripe', 'Stripe', 'https://api.stripe.com', {
  api_read: action('Read Stripe API', 'Read any JSON resource on the connected Stripe account via GET. One page only; pass Stripe pagination parameters explicitly. Key permissions apply.', 'read', { path: apiPath, parameters }, ['path'], (ctx, input) => api(ctx, input.path, 'GET', input.parameters)),
  api_write: action('Write Stripe API', 'Execute POST or DELETE on the connected Stripe account. Can charge, refund, transfer, delete or send messages: obtain authorization for the intended operation. No automatic retry; v1 DELETE ignores idempotency keys. No multipart uploads or arbitrary headers.', 'write', { path: apiPath, method: { type: 'string', enum: ['POST', 'DELETE'] }, parameters, idempotencyKey }, ['path', 'method', 'idempotencyKey'], (ctx, input) => api(ctx, input.path, input.method, input.parameters, input.idempotencyKey)),
  list_invoices: action('List invoices', 'Read one page of invoices with optional Stripe filters', 'read', { ...pagination, customerId: id() }, [], async (ctx, input) => {
    const result = await api(ctx, '/v1/invoices', 'GET', { limit: input.limit ?? 50, starting_after: input.cursor, customer: input.customerId });
    return page(result.data, result.has_more ? result.data.at(-1)?.id : null);
  }),
  get_invoice: action('Get invoice', 'Read invoice details and status', 'read', { invoiceId: id() }, ['invoiceId'], (ctx, input) => api(ctx, `/v1/invoices/${segment(input.invoiceId)}`, 'GET')),
  create_invoice: action('Create draft invoice', 'Create a draft invoice with automatic advancement disabled; excludes pending invoice items unless explicitly requested', 'write', { customerId: id(), parameters, idempotencyKey }, ['customerId', 'idempotencyKey'], (ctx, input) => api(ctx, '/v1/invoices', 'POST', { pending_invoice_items_behavior: 'exclude', ...input.parameters, customer: input.customerId, auto_advance: false }, input.idempotencyKey)),
  create_invoice_item: action('Add invoice item', 'Add an item to a specific draft invoice using Stripe price or amount/currency parameters', 'write', { invoiceId: id(), customerId: id(), parameters, idempotencyKey }, ['invoiceId', 'customerId', 'parameters', 'idempotencyKey'], (ctx, input) => api(ctx, '/v1/invoiceitems', 'POST', { ...input.parameters, invoice: input.invoiceId, customer: input.customerId }, input.idempotencyKey)),
  delete_draft_invoice: action('Delete draft invoice', 'Permanently delete a draft invoice. Stripe rejects non-draft invoices; DELETE idempotency keys do not provide deduplication in v1.', 'write', { invoiceId: id(), idempotencyKey }, ['invoiceId', 'idempotencyKey'], (ctx, input) => api(ctx, `/v1/invoices/${segment(input.invoiceId)}`, 'DELETE', {}, input.idempotencyKey)),
  ...invoiceWrites,
  list_customers: action('List customers', 'Read one page of Stripe customers', 'read', { ...pagination, email: string() }, [], async (ctx, input) => {
    const result = await request(ctx, { path: '/v1/customers', query: { limit: input.limit ?? 50, starting_after: input.cursor, email: input.email } });
    return page(result.data, result.has_more ? result.data.at(-1)?.id : null);
  }),
  get_customer: action('Get customer', 'Read one Stripe customer', 'read', { customerId: id() }, ['customerId'], (ctx, input) => request(ctx, { path: `/v1/customers/${segment(input.customerId)}` })),
  create_customer: action('Create customer', 'Create a customer using a stable idempotency key for this intended creation', 'write', { ...fields, idempotencyKey }, ['name', 'idempotencyKey'], (ctx, input) => request(ctx, { path: '/v1/customers', method: 'POST', headers: { 'Idempotency-Key': input.idempotencyKey }, form: pick(input, Object.keys(fields)) })),
  update_customer: action('Update customer', 'Update the existing customer', 'write', { customerId: id(), ...fields, idempotencyKey }, ['customerId', 'idempotencyKey'], (ctx, input) => request(ctx, { path: `/v1/customers/${segment(input.customerId)}`, method: 'POST', headers: { 'Idempotency-Key': input.idempotencyKey }, form: pick(input, Object.keys(fields)) })),
  list_subscriptions: action('List subscriptions', 'Read customer subscriptions without changing billing', 'read', { ...pagination, customerId: id() }, [], async (ctx, input) => {
    const result = await request(ctx, { path: '/v1/subscriptions', query: { customer: input.customerId, status: 'all', limit: input.limit ?? 50, starting_after: input.cursor } });
    return page(result.data, result.has_more ? result.data.at(-1)?.id : null);
  }),
}, { version: '1.2.0', description: 'Stripe account API access: customers, invoices, payments, subscriptions, refunds and other JSON resources permitted by the connection. Read/write actions are separate; writes require idempotency keys. No multipart uploads, Connect account header routing, or secret passthrough.', headers: { 'Stripe-Version': '2026-08-26.dahlia' }, verify, tokenAuth: { label: 'Stripe restricted API key (account read plus read/write permissions for the resources you use)', helpUrl: 'https://dashboard.stripe.com/apikeys' } });
