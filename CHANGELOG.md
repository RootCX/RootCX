# Changelog

Core versions. The CLI and the SDK are versioned and released separately; see
[docs/PACKAGES.md](docs/PACKAGES.md) for the package graph and release order.

Releases marked **Breaking** need the matching guide in
[docs/](docs/README.md#migration-guides) before you upgrade.

## Unreleased

## 0.28.0 - 2026-09-18 - Breaking

See [docs/migration-v028.md](docs/migration-v028.md). No runtime behavior
changes and no tenant migration runs.

- Removed the Studio desktop application, the embedded coding engine, and the
  browser crate. Development happens in external editors and coding agents
  through the CLI.
- `@rootcx/ui` moved to its own repository and is consumed as a dependency.
- `rootcx-client` dropped its `tauri` feature.
- The row-access contract is planned as SQL statements before any of it is
  applied, so a contract that cannot compile leaves the database untouched.

## 0.27.1 - 2026-09-17

- Boot no longer fails on historical database objects and stored check or index
  declarations in tenants upgraded from earlier versions.

## 0.27.0 - 2026-09-17 - Breaking

See [docs/migration-v027.md](docs/migration-v027.md).

- Governed row access: shared reads flow through assignments, and sensitive
  fields are enforced with database column privileges
  ([ADR 0006](docs/adr/0006-governed-row-access.md)).
- Resource sharing through explicit relation paths
  ([ADR 0007](docs/adr/0007-resource-sharing.md)).
- Approved backend actions, whose data access and release an administrator
  reviews ([ADR 0008](docs/adr/0008-approved-actions.md)).

## 0.26.0 - 2026-09-16

- Governed public data: a publication exposes an approved set of rows and
  fields. See [docs/publications.md](docs/publications.md).

## 0.25.0 - 2026-09-16 - Breaking

- Cross-app collection CRUD now requires a Core-approved grant naming the
  collection, its operations, and its readable and writable fields
  ([ADR 0004](docs/adr/0004-cross-app-collection-grants.md)).
- Core renews integration OAuth tokens, so rotating providers stop losing them.
- Fixed installing an array field that declares `enum_values`.

## 0.24.x - 2026-09-04 to 2026-09-08

- Bounded worker pool, supervised scheduler, and drain on shutdown.
- Idle workers are reaped again.
- A table created by a migration is governed immediately instead of at reboot.
- Closed rate-limit and invocation-check gaps on collection operations and
  self-actions.
- 5xx responses are logged, and the health probe can fail.

## 0.23.x - 2026-08-31 to 2026-09-03

- A row can be owned through a relationship, not only a column
  ([ADR 0009](docs/adr/0009-delegated-row-ownership.md)).
- Governed action and job worker transactions
  ([ADR 0002](docs/adr/0002-governed-worker-transactions.md)).
- Governance fixes: entity hooks no longer receive unreadable rows, a leading
  comment can no longer slip a privileged statement past the proxy, and an
  ownership resolver no longer answers any calling app.
- Mail transport certificate checks are enforced.

## 0.22.0 - 2026-08-07 - Breaking

See [docs/migration-v022.md](docs/migration-v022.md).

- A field can declare that it owns the row, scoping a role to its own records.
- Five endpoints reachable by any authenticated user now require a permission.
- Resumable large-file uploads and storage streaming through PostgreSQL large
  objects.

## 0.21.x - 2026-07-27 to 2026-07-30

- Official RootCX MCP server ([docs/mcp.md](docs/mcp.md)).
- Support for OpenAI-compatible gateways.
- Images prepared for restricted OpenShift environments.
- Exact decimal manifest type.

## 0.20.x - 2026-06-19 to 2026-07-13

- Durable workflow engine with schedule, record-change, and webhook triggers
  ([ADR 0001](docs/adr/0001-cross-app-automation-engine.md)).
- Worker storage download and job enqueue.
- Stable pagination with an id tie-breaker.
- Reliable long-running application exports.

## 0.19.0 - 2026-06-09 - Breaking

See [docs/migration-v019.md](docs/migration-v019.md).

- Apps lose direct database access. All SQL flows through `ctx.sql()`, which
  Core executes under Row-Level Security.

## Earlier

Releases before 0.19.0 are recorded in the git tags.
