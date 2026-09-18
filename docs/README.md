# Documentation

User guides live at [rootcx.com/docs](https://rootcx.com/docs). This directory
holds the reference material that tracks the source tree: governance guides,
migration steps, and the decision record.

## Guides

| Document | What it covers |
| --- | --- |
| [row-access.md](row-access.md) | Row ownership, shared reads, and sensitive fields enforced by RLS and column privileges |
| [resource-sharing-walkthrough.md](resource-sharing-walkthrough.md) | Sharing one exact resource, from manifest to individual revocation |
| [approved-actions.md](approved-actions.md) | Backend actions whose data access and release an administrator approves |
| [cross-app-collections.md](cross-app-collections.md) | Grants that let one app read and write another app's collection |
| [publications.md](publications.md) | Exposing an approved set of rows and fields as public data |
| [mcp.md](mcp.md) | The inbound MCP server every Core exposes |
| [PACKAGES.md](PACKAGES.md) | Package ownership, dependency graph, and release order |
| [testing.md](testing.md) | Running the Core test suite through the pinned Docker image |

## Migration guides

Read the guide for every version you cross. Breaking changes are listed in
[CHANGELOG.md](../CHANGELOG.md).

| Version | Guide |
| --- | --- |
| v0.27 | [migration-v027.md](migration-v027.md) |
| v0.22 | [migration-v022.md](migration-v022.md) |
| v0.19 | [migration-v019.md](migration-v019.md) |

## Decision records

Architecture decisions are kept in [adr/](adr/). They are append-only: a
decision that no longer holds is marked superseded rather than deleted.

| ADR | Decision | Status |
| --- | --- | --- |
| [0001](adr/0001-cross-app-automation-engine.md) | Cross-app automation engine | Accepted |
| [0002](adr/0002-governed-worker-transactions.md) | Governed worker transactions | Accepted |
| [0003](adr/0003-declared-rpc-actions-require-action-permission.md) | Declared RPC actions require their action permission | Accepted |
| [0004](adr/0004-cross-app-collection-grants.md) | Cross-app collection grants | Accepted |
| [0005](adr/0005-governed-job-provenance.md) | Core-owned job provenance | Accepted |
| [0006](adr/0006-governed-row-access.md) | Governed row access | Accepted (v0.27.0) |
| [0007](adr/0007-resource-sharing.md) | Resource sharing through relation paths | Accepted |
| [0008](adr/0008-approved-actions.md) | Approved actions | Accepted |
| [0009](adr/0009-delegated-row-ownership.md) | Delegated row ownership | Accepted, partly superseded by 0006 |

## Repository conventions

[AGENTS.md](../AGENTS.md) defines repository boundaries, [CONTEXT.md](../CONTEXT.md)
is the domain glossary, and [CLAUDE.md](../CLAUDE.md) holds the coding standards.
