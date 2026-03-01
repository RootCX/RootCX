# Packages & Architecture

## How everything connects

```
                         ┌─────────────────────────────────────┐
                         │          DEVELOPER (you)            │
                         │     Installs Studio, builds apps    │
                         └────────────────┬────────────────────┘
                                          │
                         ┌────────────────▼────────────────────┐
                         │        STUDIO (desktop IDE)         │
                         │     rootcx-studio · Tauri + React   │
                         │              internal               │
                         └──┬──────────────────────────────┬───┘
                            │                              │
                  scaffold "app"                 scaffold "agent"
                            │                              │
           ┌────────────────▼───────────┐   ┌──────────────▼───────────┐
           │       SCAFFOLDED APP       │   │     SCAFFOLDED AGENT     │
           │      Tauri + React         │   │     backend · Node/Bun   │
           │                            │   │                          │
           │  package.json              │   │  backend/package.json    │
           │  ┌──────────────────────┐  │   │  ┌────────────────────┐  │
           │  │   @rootcx/sdk        │  │   │  │  @rootcx/agents    │  │
           │  │                      │  │   │  │                    │  │
           │  │  useAppCollection()  │  │   │  │  LangGraph engine  │  │
           │  │  useAuth()           │  │   │  │  LLM providers     │  │
           │  │  AuthGate            │  │   │  │  tools (query,     │  │
           │  │  RuntimeProvider     │  │   │  │    mutate, search) │  │
           │  └──────────────────────┘  │   │  │  IPC stdin/stdout  │  │
           │  ┌──────────────────────┐  │   │  └────────────────────┘  │
           │  │   @rootcx/ui         │  │   │                          │
           │  │                      │  │   │  No frontend.            │
           │  │  Button, Card        │  │   │  No React.               │
           │  │  DataTable, AppShell │  │   │  Headless process.       │
           │  │  FormDialog, toast   │  │   │                          │
           │  └──────────────────────┘  │   └──────────────┬───────────┘
           │                            │                  │
           │  src-tauri/Cargo.toml      │                  │
           │  ┌──────────────────────┐  │                  │
           │  │   rootcx-client      │  │                  │
           │  │                      │  │                  │
           │  │  ensure_runtime()    │  │                  │
           │  │  deploy_backend()    │  │                  │
           │  └──────────┬───────────┘  │                  │
           └─────────────┼──────────────┘                  │
                         │                                 │
                    HTTP │                       IPC (JSON │ lines)
                         │                                 │
           ┌─────────────▼─────────────────────────────────▼───────────┐
           │                     CORE (daemon)                         │
           │               rootcx-core · Axum :9100                    │
           │                       internal                            │
           │                                                           │
           │  ┌─────────────────┐  ┌────────────────────────────────┐  │
           │  │  rootcx-types   │  │  rootcx-platform               │  │
           │  │                 │  │                                │  │
           │  │  AppManifest    │  │  dirs · ports · services       │  │
           │  │  OsStatus       │  │  sidecar resolution            │  │
           │  │  AiConfig       │  │  cross-platform OS utilities   │  │
           │  └─────────────────┘  └────────────────────────────────┘  │
           │                                                           │
           │  ┌─────────────────┐  ┌────────────────────────────────┐  │
           │  │  PostgreSQL     │  │  Worker supervisor             │  │
           │  │  :5480          │  │  spawn / kill agent processes  │  │
           │  └─────────────────┘  └────────────────────────────────┘  │
           └───────────────────────────────────────────────────────────┘
```

## Public packages

Installed by developers building apps on the platform.

### npm

| Package          | Source           | What it does                                            |
|------------------|------------------|---------------------------------------------------------|
| `@rootcx/sdk`    | `runtime/sdk/`   | React hooks and components to connect an app to Core    |
| `@rootcx/ui`     | `runtime/ui/`    | Shared UI component library (buttons, tables, layout)   |
| `@rootcx/agents` | `runtime/agent/` | AI agent engine — LangGraph, LLM providers, IPC bridge  |

### crates.io

| Crate              | Source                 | What it does                                       |
|--------------------|------------------------|----------------------------------------------------|
| `rootcx-client`    | `runtime/client/`      | Typed Rust HTTP client for the Core daemon         |
| `rootcx-types`     | `crates/shared-types/` | Shared types (AppManifest, OsStatus, AiConfig)     |
| `rootcx-platform`  | `crates/platform/`     | Cross-platform OS utilities (dirs, ports, services) |

### Dependency chain (publish order)

```
rootcx-types ──┐
               ├──▶ rootcx-client
rootcx-platform┘

@rootcx/sdk     (independent)
@rootcx/ui      (independent)
@rootcx/agents  (independent)
```

## Internal packages

Never published. Distributed as binaries or embedded in Studio.

| Package               | Source                   | What it does                            |
|-----------------------|--------------------------|-----------------------------------------|
| `rootcx-core`         | `core/`                  | The daemon itself — REST API, PG, supervisor. Distributed as binary. |
| `rootcx-studio`       | `studio/src-tauri/`      | Desktop IDE (Tauri 2)                   |
| `rootcx-forge`        | `crates/forge/`          | Agentic AI engine embedded in Studio    |
| `rootcx-postgres-mgmt`| `crates/postgres-mgmt/`  | Embedded PostgreSQL lifecycle           |
| `rootcx-browser`      | `crates/browser/`        | Chromium automation                     |

## Where scaffolded apps reference packages

**App frontend** — `package.json` (generated by `scaffold/layers/core.rs`):

```json
{ "@rootcx/sdk": "~0.1.0", "@rootcx/ui": "~0.1.0" }
```

**App Tauri backend** — `src-tauri/Cargo.toml` (generated by `scaffold/layers/tauri_shell.rs`):

```toml
rootcx-client = "0.1"
```

**Agent backend** — `backend/package.json` (generated by `scaffold/layers/agent.rs`):

```json
{ "@rootcx/agents": "~0.1.0" }
```
