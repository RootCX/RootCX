# Changelog

Core versions. The CLI and the SDK are versioned and released separately; see
[docs/PACKAGES.md](docs/PACKAGES.md) for the package graph and release order.

Releases marked **Breaking** need the matching guide in
[docs/](docs/README.md#migration-guides) before you upgrade.

## Unreleased

- **Breaking: `/mcp` mirrors the Core tools (ADR 0016).** It offers every
  tool the registry gives a person outside agent execution (`describe_app`,
  `query_data`, `mutate_data`, `list_apps`, `list_actions`,
  `list_integrations`, `http_request`) under the assistant rules, and each
  action an assistant may run in an installed app or integration as its own
  tool, `<app id>__<action id>`, with the action's schema and hints.
  `create_record`, `update_record` and the MCP `call_action` are removed. An
  action whose input schema cannot stand alone gets no tool.
  `get_project_context` returns each app's instructions.
- **Breaking: one business MCP surface (ADR 0015).** `/mcp` offers one fixed
  list: `get_project_context`, `describe_app`, `query_data`, `create_record`,
  `update_record`, `call_action`, `upload_file`, `validate_manifest`. `get_app`
  and `create_records` are removed; use `describe_app` and `create_record`.
  Results omit internal identities. `upload_file` also takes a file reference
  from the hosts in `ROOTCX_MCP_FILE_HOSTS`.
- **Breaking: one decision for every action call.** The RPC route, the agent
  `call_action` tool and MCP `call_action` share it: an approved action needs
  `app:{app}:action:{id}` on every entry (the agent tool accepted `invoke`), and
  only actions that ask for one get their own worker process. Listings offer
  exactly what a call would accept.
- Actions may declare `humanOnly`: no assistant (agent or MCP client) can run
  them or see them listed. Assistants cannot delete records. On MCP, a
  read-only connection may run an action declaring `readOnlyHint`, unless it is
  approved.

- A person can mark one of their own accounts as the default for an
  integration (`PUT`/`DELETE /api/v1/integrations/{id}/connections/{connectionId}/default`,
  `isDefault` in the connection list). The default serves every app and agent
  acting for that person, unless the app has its own account choice (personal or
  shared), which still wins. It is their consent, so an agent acting for them
  may use it; nobody else's calls ever reach it.
- App integration bindings now end with their app or account: uninstalling the
  app, deleting the account or disconnecting removes them in the same
  statement. Startup drops bindings already orphaned, then enforces it.
  Binding an app that is not installed is refused (409).
- `call_integration` tells the agent the person must connect the account,
  without naming the internal app id.

- `describe_app` returns the actions the caller may invoke (description and
  `inputSchema`) and the manifest `instructions`; `list_apps` lists their IDs.
  Actions follow the `call_action` permissions and are never shown through
  collection grants. Tool descriptions steer agents to actions over raw writes.

## 0.30.0 - 2026-09-23 - Breaking

See [ADR 0010](docs/adr/0010-governed-declarative-rules.md) and the updated
[docs/migration-v027.md](docs/migration-v027.md).

- Manifest `checks`, index `where` and index expressions are accepted again,
  written in a governed rule language that Core parses, type-checks and
  compiles itself; raw SQL is never executed.
- Field shorthands for single-field rules: `minimum`, `maximum`,
  `exclusive_minimum`, `exclusive_maximum`, `integer`, `max_scale`,
  `min_length`, `max_length`, `not_blank`, `format`, `pattern`, `json_type`,
  `max_items`.
- `using: "trigram"` declares trigram search indexes; `ops` and `with` remain
  refused.
- Existing rules and indexes are adopted in place when unchanged; changed rules
  are replaced without a window where they are not enforced.
- Rule and unique violations return 422 `rule_violation` and 409
  `unique_violation` instead of 500.
- **Breaking:** new submissions refuse unknown manifest keys (such as
  `onDelete` for `on_delete`, previously ignored), names longer than 63 bytes,
  and non-finite or unparsable numbers. Stored manifests still boot.

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
