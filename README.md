# RootCX

Low-code platform: build apps with AI, run them on a shared runtime.

## Architecture

```
studio/          IDE (Tauri desktop app)
  src-tauri/       Rust backend — Forge sidecar, AppRunner
  ui/              React frontend

runtime/         Daemon (Rust, axum on :9100)
  src/             Postgres lifecycle, manifest engine, Collections CRUD API
  client/          Rust HTTP client (used by Studio)
  sdk/             @rootcx/runtime — React hooks (useAppCollection, useAppRecord)
  resources/       Bundled PostgreSQL

forge/           AI agent (Python, LangGraph on :3100)

crates/
  postgres-mgmt/   Postgres process manager
  shared-types/    Types shared across Rust crates
```

## How it works

**Runtime** owns all data. It manages PostgreSQL (:5480), parses app manifests into tables, and exposes a REST API for CRUD on app collections.

**Studio** is the IDE. It talks to the Runtime daemon via HTTP. It manages the AI Forge sidecar (code generation) and AppRunner (dev builds) locally.

**Apps** are pure UI. They use `@rootcx/runtime` hooks which call the daemon via `fetch()`.

## API

| Method | Route | Description |
|--------|-------|-------------|
| GET | `/health` | Health check |
| GET | `/api/v1/status` | Runtime status |
| POST | `/api/v1/apps` | Install app (body: manifest) |
| GET | `/api/v1/apps` | List apps |
| DELETE | `/api/v1/apps/:id` | Uninstall app |
| GET | `/api/v1/apps/:id/collections/:entity` | List records |
| POST | `/api/v1/apps/:id/collections/:entity` | Create record |
| GET | `/api/v1/apps/:id/collections/:entity/:rid` | Get record |
| PATCH | `/api/v1/apps/:id/collections/:entity/:rid` | Update record |
| DELETE | `/api/v1/apps/:id/collections/:entity/:rid` | Delete record |

## Dev

```bash
cargo run -p rootcx-runtime   # start daemon on :9100
cargo tauri dev --manifest-path studio/src-tauri/Cargo.toml  # start Studio
```
