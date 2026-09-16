# Core testing

Requires Rust/Cargo, Bun and a running Docker engine.

Use Cargo for Rust and `bun test` for the worker prelude. Make targets only
orchestrate these existing runners; testcontainers supplies real PostgreSQL.
No additional runner or coverage framework is required.

The SDK and Peppol integration separately use Vitest. Those package tests are
outside this Core gate; their presence does not add a runtime to the RLS/CRUD
test path. See `runtime/sdk/src/*.test.ts` and
`core/resources/integrations/peppol/*.test.ts`.

## Commands

| Command | Purpose |
| --- | --- |
| `make core-verify` | Library tests, governance integration tests, Bun prelude, whitespace check |
| `make core-unit` | All library tests, including their SQL/provider boundary tests, with one disposable database |
| `make core-governance` | Cross-app CRUD, lifecycle, metadata, delegation, identity, ownership and workflows |
| `make core-governance FILTER=cross_app_lifecycle_test::revoke_waits` | One integration behavior |
| `cargo test -p rootcx-core --lib governance::cross_app::tests` | Fast grant validator/projection tests; no database needed |
| `bun test core/src/backend_prelude.test.ts` | Real worker subprocess protocol tests |
| `make core-check-tests` | Type-check every Core test target, including suites outside the governance release gate |

`TEST_THREADS=2` bounds concurrent integration containers by default.
`CARGO_JOBS` separately controls compilation parallelism. `core-verify` clears
the integration name filter so an inherited `FILTER` cannot silently narrow
the release gate.

`core-verify` compiles the tests it executes. It does not first run a redundant
`cargo check` build. It is a Core governance gate, not the entire repository's
test suite or a production performance benchmark.

## Isolation and cost

Each integration test gets its own PostgreSQL container, data directory, Core
runtime, HTTP server and real Bun workers. This preserves isolation of SQL
roles, migrations, grants and ownership policies. Product errors are not retried.
The harness has a bounded retry only for Docker failing to publish a port,
before Core starts.

The governance suites live in `core/tests/governance/` and share one native
Cargo entry point, `governance_test.rs`. Their names remain usable as filters.
This compiles the harness once and avoids repeated executable startup. It
does not share databases or runtimes between tests.

Every fixture-owning governance test awaits `rt.shutdown()`. The harness stops
its HTTP server, drains Core workers and closes the database pool before the
container is removed. Omitting shutdown previously left PostgreSQL sockets
open in the shared process; separate test binaries had hidden that accumulation.
`TEST_TIMINGS=1` also reports open descriptors where `/dev/fd` is available.

The seeded assistant is a committed, dependency-free worker fixture. Core
still registers and starts it normally; tests do not download GitHub's latest
assistant release, run its package installation, or contact an LLM.
The production assistant download remains the default outside the harness.
Loopback harness requests explicitly bypass host proxy discovery.

Library SQL tests share bootstrap completion, but each gets a pool belonging
to its own Tokio runtime. `TEST_DATABASE_URL` is mandatory for these tests.
`make core-unit` creates and removes a fresh database container and runs the
library suite once; it never falls back to a developer's database.

Bun tests terminate and await their fixture processes, including after failed
assertions. Already-exited processes no longer incur a teardown timeout.

For per-phase integration timings:

```sh
TEST_TIMINGS=1 make core-governance FILTER=cross_app_metadata_test
```

Record run-specific counts, timings and release blockers in the PR or issue,
not in this guide. Test timings exclude compilation and are not production
latency measurements.

## Governance coverage by invariant

| Invariant / realistic regression | Primary checks |
| --- | --- |
| Empty, unknown or wildcard actions fail closed; field projections reject unknown, sensitive and system write fields | `governance::cross_app::tests` |
| Query limits reject malformed/out-of-range inputs; new fields cannot leak through frozen projections | `governance::cross_app::tests`, `cross_app_grants_test` |
| Local/remote `find` and no-options `query_data` return more than 1000 visible rows without truncation; explicit pages retain bounds, RLS, totals and projection | `collection_reads_test` |
| Local mutations preserve both update forms; deletion requires a UUID and neither operation can change an invisible row | `collection_reads_test` |
| Each CRUD action requires its own grant; writes obey ownership and writable fields | `cross_app_crud_test`, `cross_app_lifecycle_test` |
| Failure to persist success audit rolls back the business write | `cross_app_crud_test` |
| Revocation waits for in-flight transactions; saved authority and old installation generations fail | `cross_app_lifecycle_test` |
| Cosmetic metadata preserves grants and ownership; contract edits still revoke, including after failed updates | `cross_app_lifecycle_test`, `manifest::tests` |
| Grant transitions and installation locks avoid deadlock and stale contract approval | `cross_app_lifecycle_test` |
| Expiry is rechecked after lock waits; replacing expired grants and recording history are atomic; uninstall records its initiating actor | `cross_app_lifecycle_test` |
| A record's JSON array field cannot corrupt audit row counts | `cross_app_grants_test` |
| Task scope and delegation remain permission ceilings, including mutation-only grants | `cross_app_delegation_test`, `delegation_matrix_test` |
| Queued workers retain their delegated ceiling and respect current revocation; payloads cannot select native workflow execution | `cross_app_jobs_test` |
| Agent enqueue without invocation scope is denied; credential and audit chains cannot share a worker | `cross_app_jobs_test`, `worker_manager::tests` |
| Discovery matches executor capabilities; unavailable workflow tools fail at save; authenticated HTTP data tools retain RLS; missing responsible human cannot become remote actor authority | `tool_availability_test` |
| Raw worker SQL cannot impersonate human HTTP or bypass app boundaries; agents cannot bypass tool dispatch | `cross_app_delegation_test`, `governance_contract_test` |
| Metadata is permission-filtered and omits sensitive schema details | `cross_app_metadata_test`, `cross_app_grants_test` |
| Own-scoped rows follow direct, chained and core-user entity links without widening writes | `row_ownership_test` |
| Identity, supervision, triggers and workflow CRUD enforce their real process/SQL boundaries | `agent_identity_test`, `governance_contract_test`, `workflows_integration` |
| Shared local/remote collection signatures preserve both update forms, route equality versus page requests, and reject transaction misuse | `backend_prelude.test.ts`, `cross_app_crud_test` |

The count of tests is not a coverage percentage. No line/branch coverage
percentage or exhaustive security guarantee is claimed. New tests should name
the product mistake they catch, parameterize equivalent inputs, and keep
security outcome assertions visible. Pure validation cases belong in unit
tests; RLS, transaction races, rollback and IPC belong at real boundaries.
