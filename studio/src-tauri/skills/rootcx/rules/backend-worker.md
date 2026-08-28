# RootCX Backend Workers

Apps can have a `backend/` directory with a Bun worker for server-side logic. Core manages lifecycle (spawn, crash recovery, shutdown). IPC via JSON-lines on stdin/stdout.

Deps: add `backend/package.json` for backend-only npm deps. Core runs `bun install` there at deploy. Do NOT put backend deps in the root `package.json` (that one is for the frontend/Vite).

## Governed runtime

Use the Core-injected `serve()` and `ctx` Interface. A v3 worker never parses
JSON-lines itself and never receives a database URL, user token, or forgeable
identity context.

## Data access

- **Independent CRUD**: use `ctx.collection(entity)`.
- **One SQL statement**: use `ctx.sql(text, params)`.
- **Atomic workflow**: use `ctx.transaction(async tx => ...)` and only `tx.sql` inside.
- SQL always inherits the worker's fixed identity, RLS, audit context, scoped search path and restricted role.
- **NEVER use SQLite or file-based storage** — PostgreSQL is the only database

## Frontend → Worker

```tsx
const client = useRuntimeClient();
const result = await client.rpc(appId, "method_name", { ...params });
```

## Callback transactions

```typescript
serve({ rpc: {
  createOrder: async (params, _caller, ctx) =>
    ctx.transaction(async (tx) => {
      const order = await tx.sql(
        "INSERT INTO orders (number) VALUES ($1) RETURNING id", [params.number],
      );
      await tx.sql("INSERT INTO order_lines (order_id, article_id) VALUES ($1, $2)", [
        order.rows[0][0], params.articleId,
      ]);
      return { id: order.rows[0][0] };
    }),
} });
```

Commit occurs only when the callback and every statement succeed. Statement
calls are serialized in call order. Any statement error poisons the transaction,
even if caught. Nesting and use of collection/integration/storage/event/job
capabilities inside the callback are rejected. Core enforces 8 seconds per
statement, 1,000 returned rows, 30 seconds idle and a 60-second resource ceiling.

## Rules

- Entry point: `index.ts` → `index.js` → `main.ts` → `main.js` → `src/index.ts`
- RPC timeout: 30s; keep interactive transactions below it.
- Put external effects before/after the transaction, or persist an outbox row.
- Crash recovery: max 5 crashes in 60s → failed state
