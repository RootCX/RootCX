---
name: rootcx-sdk-hooks
description: Querying or mutating RootCX data from React using @rootcx/sdk hooks — useAppCollection, useAppRecord, useIntegration, useCoreCollection, useRuntimeClient. Covers QueryOptions, where operators, cross-app reads, and the flat record shape.
version: 0.1.0
---

# RootCX SDK Hooks

All data from hooks. Types exported from `@rootcx/sdk`. Never use `useState` with mock data.

## useAppCollection

```tsx
useAppCollection<T>(appId, entity, query?: QueryOptions)
```

Returns: `{ data: T[], total: number, loading, error, refetch, create, bulkCreate, update, remove }`

Without `query`: `GET /collections/{entity}` (full list). With `query`: `POST /collections/{entity}/query` (server-side filter/sort/paginate). Auto re-fetches on `query` change.

**Cross-app reads:** `appId` can be any installed app or integration — not limited to the current app. Use another app's ID to read its collections. User must have read permissions on the target app.

`create(fields) → T` · `bulkCreate(fields[]) → T[]` · `update(id, fields) → T` · `remove(id) → void`

## QueryOptions

```tsx
{ where?: WhereClause, orderBy?: string, order?: "asc"|"desc", limit?: number, offset?: number }
```

**Where operators:** `$eq` `$ne` `$gt` `$gte` `$lt` `$lte` `$like` `$ilike` `$in` `$nin` `$contains` `$isNull`
**Logical:** `$and` `$or` (WhereClause[]) · `$not` (WhereClause)
**Shorthand:** `{field: value}` = `{field: {$eq: value}}` · `{field: null}` = IS NULL

```tsx
useAppCollection<Invoice>(appId, "invoice", {
  where: { $or: [{status: "pending"}, {amount: {$gte: 1000}}], date: {$gte: "2026-01-01", $lte: "2026-03-31"}, name: {$ilike: "%acme%"} },
  orderBy: "date", order: "desc", limit: 50, offset: 0,
});
```

## useAppRecord

```tsx
useAppRecord<T>(appId, entity, recordId | null)
```

Returns: `{ data: T|null, loading, error, refetch, update, remove }`

`update(fields) → T` · `remove() → void` · `null` id skips fetch

## useIntegration

```tsx
const { connected, loading, connect, submitCredentials, disconnect, call } = useIntegration(integrationId);
```

`connect()` → OAuth redirect or `{type:"credentials", schema}` · `call(actionId, params?) → result`

**Call `list_integrations` first. Never guess action IDs.**

## useCoreCollection

```tsx
useCoreCollection<T>(entity)
```

Returns: `{ data: T[], loading, error, refetch }`

Read-only access to core platform entities. `GET /api/v1/{entity}` (not app collections).

**`core:users` in manifest `entity_link` references → use `useCoreCollection("users")` to fetch org members. Do NOT use `useAppCollection` with `core:users` — it will 404.**

## useRuntimeClient

```tsx
const client = useRuntimeClient();
```

`client.queryRecords<T>(appId, entity, QueryOptions) → {data, total}` · `client.rpc(appId, method, params?) → unknown` · `client.core().collection<T>(entity).list() → T[]` · `client.core().collection<T>(entity).get(id) → T`

For imperative calls in event handlers. For reactive data, use `useAppCollection` / `useCoreCollection`.

---

## Record shape

Records are **flat objects**. Auto-fields: `id`, `created_at`, `updated_at`.
When creating/updating, pass only user-defined fields.
