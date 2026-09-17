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

Ordinary authenticated declared RPCs accept `app:{appId}:invoke` **OR**
`app:{appId}:action:{method_name}`. Undeclared internal RPCs require `invoke`.
Actions declaring `authority` require the fine action grant **and** current
operator approval; plain `invoke` is insufficient. Public share-token and
anonymous RPCs are
authorized by `manifest.public.rpcs` and its scope rules instead of user action
permissions.

For governed SQL, Core derives immutable invocation metadata and sets it before
the restricted database role is assumed. Application code cannot set it:

- approved action or ordinary action with `isolatedScope: true`:
  `rootcx.invocation_kind = 'action'`, with the action ID in
  both `rootcx.invocation_name` and `rootcx.action_id`;
- trusted scheduled job with `isolatedScope: true`:
  `rootcx.invocation_kind = 'job'`, with the declared
  `cron_schedules.name` in `rootcx.invocation_name` and an empty
  `rootcx.action_id`;
- nonisolated action, internal/public RPC or ordinary queued job: all three
  values are empty.

Never derive workflow authority from RPC parameters or job payload fields.
Do not install app-authored triggers or policies to interpret these settings;
SQL admission rejects unsupported SQL artifacts. Business orchestration remains
in TypeScript, within ordinary caller permissions or explicitly approved
authority.

## Approved backend actions

For action-only workflows, declare `actions[].authority.data` with exact local
entity names and CRUD verbs, then obtain administrator approval of the complete
backend release. Users receive the fine action grants, not collection CRUD.
Core uses the approved ceiling instead of adding the caller's data permissions.
Use lowercase snake_case action IDs in the manifest, `serve` handler names,
grants and calls; uppercase fails permission-key validation. Keep API/body keys
such as `caller.userId`, `caseId` and `backendDigest` unchanged.

The per-approval PostgreSQL role and RLS enforce denied table/verb access with
database errors. Foreign-key cascade/set-null/set-default writes
must be explicitly included for every affected local entity and verb;
otherwise approval is rejected. Cross-app effects cannot be approved.

Read the [complete executable recipe](../../../docs/approved-actions.md) before
building one. It uses one case shared by two business assignments, explicitly
projects away private notes, and checks `caller.userId` in parameterized SQL.
A guarded create must lock an active assignment with `SELECT ... FOR SHARE`
inside `ctx.transaction` before inserting. Include assignment `read` and
target `read` when using `RETURNING`; do not request assignment `update` just
for locking. Core's technical primary-key UPDATE ACL and RLS `USING` permit
the read lock, while `WITH CHECK` denies actual updates without declared
`update` authority, including primary-key changes.
Business revocation updates/deletes the same locked assignment row; approval
revocation is a separate Core barrier waiting for admitted DB transactions.

The approver trusts the **full artifact and resolved dependencies**, including
initializers and every handler sharing the runtime, not just the chosen function.
Core hashes after dependency installation and records a versioned snapshot,
verifying its hash at approval and approved-process startup. Read-only settings
are defensive, not immutability against hostile processes under the same OS
account or protection against interference after verification.
Every backend deployment and every full manifest change, including cosmetics,
rotates revision and invalidates approval; old approvals never revive.
`rootcx apps actions list <app> --json` exposes the exact review.
`approve <app> <action>` prompts with that snapshot; `--yes` also requires
`--revision`, `--digest`, and `--installation`. `revoke <app> <action>` removes it.
These subcommands all follow `rootcx apps actions`; deployment never approves.
Invocation without current approval returns HTTP 403; a stale approval POST
returns HTTP 409.

Only exact local CRUD through collections/SQL/callback transactions is supported.
Sensitive reads remain forbidden. No jobs, remote calls, self-action calls,
tools, integrations, storage, events or injected credentials are available,
including before/after the transaction. Approved execution uses the usual
cross-platform Bun spawning path and a fresh process per invocation, stopped
after use and never returned to an ordinary worker cache. It adds no
OS-identity or volume-ownership prerequisite.
Ordinary apps remain untrusted at the governed IPC/SQL layer; approved full
artifacts are trusted for business checks. Host filesystem/process confinement
is outside the existing deployment model. Do not claim complete malicious-app
confinement or platform test coverage without evidence from actual runs.

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
- In ordinary workers, do integrations, storage, events and jobs before/after
  the transaction, or use an outbox. Approved actions have none of these capabilities.
- Crash recovery: max 5 crashes in 60s → failed state
