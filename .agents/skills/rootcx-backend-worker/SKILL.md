---
name: rootcx-backend-worker
description: Writing a governed RootCX Bun worker with serve(), typed ctx capabilities, callback transactions, RPC methods and jobs. Load when implementing an app backend/index.ts.
version: 0.3.0
---

# RootCX Backend Workers

Apps can have a `backend/` directory with a Bun worker for server-side logic. Core manages lifecycle (spawn, crash recovery, shutdown). IPC via JSON-lines on stdin/stdout.

Deps: add `backend/package.json` for backend-only npm deps. Core runs `bun install` there at deploy. Do NOT put backend deps in the root `package.json` (that one is for the frontend/Vite).

## Governed runtime

Use the Core-injected `serve()` and `ctx` Interface. Never parse raw IPC and
never connect directly to PostgreSQL. Workers receive no database URL or token.

## Data access

- **Simple CRUD**: `ctx.collection(entity)`
- **One statement**: `ctx.sql(text, params)`
- **Atomic workflow**: `ctx.transaction(async tx => ...)`, using only `tx.sql`
- **NEVER use SQLite or file-based storage** — PostgreSQL is the only database

## Frontend → Worker

```tsx
const client = useRuntimeClient();
const result = await client.rpc(appId, "method_name", { ...params });
```

## Minimal worker template

Core shape:

```typescript
serve({ rpc: {
  create: async (params, _caller, ctx) => ctx.transaction(async (tx) => {
    const row = await tx.sql("INSERT INTO items (name) VALUES ($1) RETURNING id", [params.name]);
    return { id: row.rows[0][0] };
  }),
} });
```

## Rules

- Entry point: `index.ts` → `index.js` → `main.ts` → `main.js` → `src/index.ts`
- RPC timeout: 30s. Always respond with matching `id`
- Any transaction statement error forces rollback, even if application code catches it.
- Do integrations, storage, events and jobs before/after the transaction, or use an outbox.
- Crash recovery: max 5 crashes in 60s → failed state
