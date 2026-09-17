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

Ordinary declared actions accept `app:{appId}:invoke` OR the fine
`app:{appId}:action:{id}` grant; undeclared RPCs require `invoke`. Actions with
`authority` require the fine grant and current operator approval. Plain
`invoke` is insufficient.

## Approved backend actions

Use the [manifest rule](manifest.md) to request exact local CRUD authority.
Core preserves permission/IPC checks, the per-approval PostgreSQL role and RLS,
transaction controls and the revocation barrier. Caller data permissions do
not widen that ceiling. Sensitive reads remain forbidden; jobs, remote or
self-action calls, tools, integrations, storage, events and injected credentials
are unavailable throughout approved execution.

Each approved invocation uses a fresh process through the usual cross-platform
Bun spawning path, bound by Core to its caller, action and approval, then stopped
after use. It adds no OS-identity or volume-ownership prerequisite.
The approver trusts the full artifact, dependencies, initialization and every
handler sharing that runtime for business checks and output projection.
Versioned snapshots are hashed after dependency installation and verified at
approval and approved-process startup. Read-only settings are defensive, not
immutability against hostile processes under the same OS account or protection
against interference after verification.

Ordinary apps remain untrusted at governed IPC/SQL boundaries. Host filesystem
and process confinement are outside the existing deployment model; do not
claim complete malicious-app confinement or platform test coverage from the
spawning architecture alone.

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
- In ordinary workers, put external effects before/after the transaction, or
  persist an outbox row. Approved actions have no external-effect capabilities.
- Crash recovery: max 5 crashes in 60s → failed state
