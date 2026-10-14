import { IntegrationError } from './http';
import type { Schema, Action, ObjectValue, Context, Effect } from './types';

export const string = (description = '', extra: Schema = {}): Schema => ({ type: 'string', minLength: 1, maxLength: 4096, description, ...extra });
export const id = (description = 'Provider identifier'): Schema => string(description, { pattern: '^[A-Za-z0-9_-]+$', maxLength: 256 });
export const integer = (minimum: number, maximum: number): Schema => ({ type: 'integer', minimum, maximum });
export const boolean: Schema = { type: 'boolean' };
export const map: Schema = { type: 'object', maxProperties: 200, additionalProperties: true };
export const array = (items: Schema, maxItems = 100): Schema => ({ type: 'array', items, maxItems });
export const object = (properties: Record<string, Schema>, required: string[] = []): Schema => ({ type: 'object', properties, required, additionalProperties: false });
export const pagination = { limit: integer(1, 100), cursor: string('Opaque cursor from the previous response', { maxLength: 8192 }) };
export function action(name: string, description: string, effect: Effect, properties: Record<string, Schema>, required: string[], run: (ctx: Context, input: ObjectValue) => Promise<unknown>): Action {
  return { name, description, effect, input: object(properties, required), output: { type: 'object' }, run };
}
export const segment = (value: string) => encodeURIComponent(value);
export function page(items: unknown[], cursor?: string | null) {
  if (!Array.isArray(items) || (cursor != null && typeof cursor !== 'string')) throw new IntegrationError('INVALID_RESPONSE', 'Provider returned an invalid page');
  return { items, nextCursor: cursor || null };
}
