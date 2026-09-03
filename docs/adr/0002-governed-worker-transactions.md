# ADR 0002: Governed callback transactions for workers

Status: Proposed (2026-08-28)

## Context

`ctx.sql` executes one governed PostgreSQL transaction per statement. Applications
that require several atomic writes therefore had to hide orchestration in large
PL/pgSQL functions. The Core already contained transaction sessions and IPC
messages, but they were not exposed by the JavaScript runtime and remained on
protocol v2 despite the protocol's additive-version rule.

The Core is the governance authority. A public transaction Interface must not
give an application a connection, identity token, role control, transaction
identifier, or manual lifecycle control.

## Decision

1. Worker protocol v3 exposes one deep Interface:
   `ctx.transaction(async tx => { ... })`.
2. The transaction Adapter exposes only `tx.sql`. Begin, commit, rollback and
   the Core-generated transaction identifier remain private.
3. The Core commits only after the callback and every statement succeed. The
   first validation, binding, execution, rate-limit or serialization error
   poisons the session and makes commit impossible.
4. Statements are queued in IPC arrival order by the Core and in call order by
   the JavaScript Adapter. Concurrent `tx.sql` calls are therefore deterministic.
5. Multiple RPC handlers may hold separate transactions in one identity-bound
   worker. A per-worker cap of four and the process-global cap of eight protect
   the shared connection pool.
6. Every session inherits the worker's immutable identity, RLS context, audit
   context, app-scoped search path, restricted executor role and SQL validator.
7. Transaction handles expire with the callback. Nested transactions and use of
   ambient database/external capabilities in the callback are rejected by the
   public Adapter. External effects use a post-commit step or transactional outbox.
8. The resource lifetime is bounded from the start of acquisition: 60 seconds
   total, 30 seconds idle, and 8 seconds per statement. Interactive RPCs retain
   their independent 30-second timeout.

## Consequences

- Business orchestration can live in TypeScript without sacrificing atomicity.
- Database constraints, RLS and relational invariants remain in PostgreSQL.
- Existing v1/v2 workers remain compatible: they never send transaction
  messages, so they are simply never offered `ctx.transaction`. A worker that
  announces no version, or a version below v3, keeps every capability it had
  before this ADR — an announced version unlocks a capability, it never gates
  one a worker already had. (A 2026-08-28 change briefly refused transaction
  and declared-action messages from any worker below the relevant version
  instead of degrading, which took every write and every declared action down
  on every worker predating the version field; corrected the same release.)
- Applications cannot recover and commit after any statement error. Retrying
  requires a new callback transaction.
- Collections, integrations, storage, events and jobs are deliberately not
  presented as transactionally atomic.
