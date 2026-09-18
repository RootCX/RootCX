# Packages & Architecture

## How everything connects

Developers build apps locally with Codex, Claude Code, Cursor, or their editor.
The RootCX CLI scaffolds React apps, builds them locally, and deploys them to Core.

```text
Editor / coding agent
        │
        ▼
RootCX CLI ── rootcx-scaffold
        │
        ├── React app: @rootcx/sdk + @rootcx/ui
        └── Optional backend: worker or LangGraph agent
        │
        ▼
Core: REST API, authentication, RBAC, audit, PostgreSQL, worker supervisor
```

Agents are apps with a `backend/` containing a LangGraph agent. They use the same
deployment and governance as other apps. Core supervises workers through JSON-line
IPC; frontends use the HTTP SDK.

## npm packages

| Package | Source | Purpose |
| --- | --- | --- |
| `@rootcx/sdk` | `runtime/sdk/` | React hooks, authentication and the Core HTTP client |
| `@rootcx/ui` | Separate `rootcx-ui` repository | RootCX theme and composable shadcn/ui components built with Radix and Tailwind CSS 4 |

`rootcx-ui` owns UI sources, the theme, npm packaging and the shadcn registry.
This repository consumes the published package; it does not contain a second UI
library. Apps import `@rootcx/ui/theme.css` and compose the package's components.
Routes, form validation, status meanings and table behavior belong to the app.

The new scaffold requires `@rootcx/ui` 0.9 and the browser-only `@rootcx/sdk` 0.19.
Publish the UI from `rootcx-ui` and the SDK from `runtime/sdk` before releasing the
CLI that generates these apps. For local verification, pack the UI repository and run:

```sh
node scripts/check-ui-consumer.mjs /absolute/path/to/rootcx-ui-0.9.0.tgz
```

This builds the simple, authenticated and agent scaffolds plus the output of the
compiled CLI's `rootcx new`. It checks the declared UI version before any local
override, the package exports, TypeScript, and Tailwind output for the UI.
Archive mode also builds and packs the local SDK,
and checks that the installed SDK has no desktop bridge.

An archive check does not prove the release is usable from npm. Before releasing
the CLI, run the same check without dependency overrides:

```sh
node scripts/check-ui-consumer.mjs --registry
```

For browser verification, run `pnpm exec vite preview --host 127.0.0.1 --port PORT`
inside the generated `simple`, `auth`, `agent` and `cli` fixtures, respectively on
ports 5891, 5892, 5893 and 5894. Then run from this repository:

```sh
playwright-cli -s=ui-audit open http://127.0.0.1:5891
playwright-cli -s=ui-audit run-code --filename=scripts/check-ui-browser.js
playwright-cli -s=ui-audit close
```

The browser check verifies the package's computed theme, mobile controls,
authentication form composition, registration, errors, logout and SSO navigation.
Core responses are intercepted fixtures, not calls to a live workspace.

## Rust packages

| Package | Source | Purpose |
| --- | --- | --- |
| `rootcx-core` | `core/` | Core server, distributed as a binary and Docker image |
| `rootcx-cli` | `crates/rootcx-cli/` | Local scaffolding, authentication and deployment |
| `rootcx-scaffold` | `crates/rootcx-scaffold/` | React app and backend templates used by the CLI |
| `rootcx-client` | `runtime/client/` | Typed Rust HTTP client for Core |
| `rootcx-types` | `crates/shared-types/` | Shared manifests, status and protocol types |
| `rootcx-platform` | `crates/platform/` | Platform utilities for directories, binaries, ports and services |

`rootcx-client` depends on `rootcx-types`; publish shared types first when
releasing the Rust client. The npm packages have independent release cycles.

## Generated apps

`crates/rootcx-scaffold/src/layers/core.rs` defines frontend dependencies and
configuration. Its templates import `@rootcx/ui/theme.css` once and mount
`TooltipProvider` and `Toaster`. The theme is light.

The authentication layer composes `AuthGate` from `@rootcx/sdk` with an app-local
`AuthForm` using `@rootcx/ui` cards, fields, inputs and buttons. Both authenticated
apps and agents use that form and the package's loading indicator. The SDK owns
authentication state and submission; the app owns presentation.
SDK 0.19 requires the form slot and no longer embeds a styled fallback form.

The agent layer adds app-local chat components and a provider-specific backend.
No Rust or desktop wrapper is required in generated apps.
