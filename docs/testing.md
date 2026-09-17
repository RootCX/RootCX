# Core testing

Run `make test`. Only Docker Compose is required for the test runtime: Rust,
Bun and cargo-nextest run in the pinned test image. No host Rust build artifacts
are read or deleted.

The current PostgreSQL image is ARM64-only, so CI uses a native ARM64 runner.
An AMD64 host needs ARM64 container emulation to run this image.

## Commands

| Command | Purpose |
| --- | --- |
| `make test` / `make core-verify` | Library tests, governance integration tests, Bun prelude, whitespace check |
| `make core-unit FILTER=publications` | Library tests matching a name |
| `make test-integration FILTER=publications_test` | One integration suite |
| `make core-test TEST=api_integration FILTER=cron` | A suite outside the governance gate |
| `make core-check-tests` | Native type-check of every Core test target |
| `make core-mutations` | Isolated row-access security mutants; passing baselines and assertion evidence required |

Compilation and execution are separate phases. The first build downloads and
compiles dependencies; subsequent runs reuse the Docker volume
`rootcx-core-test-cache`. `CARGO_JOBS` defaults to 2. Do not delete this cache as
part of a normal test run. `core-verify` ignores `FILTER` so a release check cannot
silently run a subset.

## Isolation, limits and cleanup

Each invocation creates one private Compose project with one PostgreSQL server.
It never connects to the development database or exposes PostgreSQL on a host
port. The PostgreSQL container is removed on success, failure or interruption;
only the Rust build cache persists.

Integration tests run sequentially through nextest. Each Core fixture acquires a
PostgreSQL advisory lock and recreates `rootcx_test` before booting. Its connection
holds the lock until shutdown. Even direct parallel invocation of the Rust test
binary cannot reset another fixture's database. Each fixture still has a fresh
Core runtime, HTTP server, app data directory and real Bun workers.

This serialization is intentional: pg_cron supports one installation database
per PostgreSQL server. Independent concurrent databases would disable or fake
cron behavior. We keep the real pg_cron and pgmq extensions, and reset their data
between tests. The shared executor role is bootstrapped idempotently.

Library tests run once with Cargo's test harness and share their disposable
database. Their pools belong to each test's Tokio runtime. The seeded assistant
is a committed worker fixture; tests do not download an assistant or call an LLM.

Limits are explicit: compilation 20 minutes, library tests 5 minutes, integration
suite 20 minutes, each integration test 2 minutes, and Bun tests 2 minutes.
Nextest does not retry failures. The test container owns all workers, so removing
it also removes subprocesses after a panic or timeout. Infrastructure and build
failures are reported before test execution; they are not passing test results.

Run `make core-verify` before publishing a Core release.
The Core gate also includes lifecycle authority and app migration refusal tests.
`core-mutations` edits only the disposable image's source copy, restores each
mutant, and reports compile errors, infrastructure failures and survivors
separately from killed mutants. `FILTER=mutant-id` selects one mutation and still
requires its passing baseline. Run it separately from `core-verify`.

The SDK and Peppol Vitest suites are separate from this Core gate. Run-specific
counts and timings belong in the PR, not in this guide.

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
| Assignment access is many-to-many, non-transitive and revoked on the next statement, including Bun callback transactions | `assignment_access_test` |
| Resource sharing confines reads to exact roots and explicit targets, including owner-free resources; public role/assignment operations revoke one reader without affecting another; resolver guards and legacy contracts remain intact | `resource_sharing_test` |
| Sensitive values cannot be read through raw SQL, expressions or predicates; generated responses remain usable | `sensitive_sql_test` |
| Invalid projections fail visibly and atomically; no-share policies retain historical definitions | `row_access_projection_test` |
| Untrusted SQL and executable legacy artifacts cannot enter privileged schema operations | `manifest_sql_admission_test`, `app_migrations_test` |
| Lifecycle and anonymous workers cannot manufacture assignments | `worker_lifecycle_test` |
| Identity, supervision, triggers and workflow CRUD enforce their real process/SQL boundaries | `agent_identity_test`, `governance_contract_test`, `workflows_integration` |
| Shared local/remote collection signatures preserve both update forms, route equality versus page requests, and reject transaction misuse | `backend_prelude.test.ts`, `cross_app_crud_test` |

Mutation builds use a separate Cargo target cache and rebuild the Core package
before their baseline; mutant executables cannot populate the normal test cache.

The count of tests is not a coverage percentage. No line/branch coverage
percentage or exhaustive security guarantee is claimed. New tests should name
the product mistake they catch, parameterize equivalent inputs, and keep
security outcome assertions visible. Pure validation cases belong in unit
tests; RLS, transaction races, rollback and IPC belong at real boundaries.
