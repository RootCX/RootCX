# Migration guide: Core v0.28.0 — Studio removed, development moves to the CLI

This release removes the Studio desktop application. Core itself gains no new
capability and changes no runtime behavior: a tenant on 0.27.1 upgrades without
data migration, manifest changes, or redeployment of its apps.

What changes is how you develop against Core, and which packages you depend on.

## Compatibility changes

| Existing behavior or artifact | Behavior in 0.28.0 |
| --- | --- |
| Studio desktop application | Removed. Develop in your own editor or coding agent and deploy through the CLI |
| Embedded coding engine and browser crate | Removed with Studio; no replacement ships in this repository |
| `@rootcx/ui` | Published from its own repository. Consume it as a dependency; shared component and theme changes belong there |
| `rootcx-client` `tauri` feature | Dropped. Remove the feature from any `Cargo.toml` that names it |
| Installed apps, manifests, permissions, data | Unchanged. No migration runs and no redeployment is required |
| REST API, IPC protocol, agent and worker behavior | Unchanged |

## Upgrading

1. Upgrade the Core image to `0.28.0`. No schema migration is performed for this
   release beyond the idempotent boot reconciliation Core already runs.
2. Install the CLI if you were driving deployments from Studio. See
   [PACKAGES.md](PACKAGES.md) for the package graph.
3. In any app that depended on `rootcx-client`'s `tauri` feature, drop it.
4. Point `@rootcx/ui` at the published package rather than a workspace path.

## Rolling back

0.28.0 makes no irreversible change to a tenant's database, so rolling back to
0.27.1 is a matter of redeploying the previous image.

## Internal Rust API

`rootcx-core` is distributed as a binary, not as a published crate, so this
affects nobody consuming a release. For completeness: the governed transaction
constructors `begin_app_tx`, `begin_app_tx_with_invocation`,
`begin_app_tx_with_invocation_and_cross_app` and `begin_human_tx` are replaced by
a single `DataAccess` request, and `set_rls_context` is now private.
