# RootCX

Low-code platform: build apps with AI, run them on a shared runtime.

## Architecture

```
studio/          IDE (Tauri + React) — extension-first
  ui/src/
    core/          Extension host API (~100 LOC)
    extensions/    Built-in extensions (each a self-contained directory)
  src-tauri/       Rust backend — Forge sidecar, PTY, native menu

runtime/         Daemon (Rust, axum on :9100)
  src/             Postgres lifecycle, manifest engine, REST API
  client/          Rust HTTP client (used by Studio)
  sdk/             @rootcx/runtime — React hooks for apps

forge/           AI agent (Python, LangGraph on :3100)

crates/
  postgres-mgmt/   Postgres process manager
  shared-types/    Types shared across Rust crates
```

## Studio Extension System

Studio is **extension-first**. Every panel, command, and status bar item is an extension — built-in or community. The core is a generic `Registry<T>` primitive (~30 LOC) that powers all contribution types.

### Core API (`core/`)

```typescript
import { views, commands, statusBar, workspace, layout } from "@/core";

views.register("my-panel", { title, icon, defaultZone, component });
commands.register("my-cmd", { title, handler });
statusBar.register("my-item", { alignment, priority, component });
```

Adding a new contribution type = one line: `export const themes = new Registry<Theme>()`.

### Extension structure

Each extension is a self-contained directory:

```
extensions/
  explorer/          File browser
    index.ts           activate() — registers views, commands
    panel.tsx          React component
  forge/             AI code generation
  console/           Terminal (xterm.js + PTY)
  welcome/           Dashboard
  output/            Build output
  core-status/       Service status dots
  run/               F5 / Run Project command
  activate.ts        Loads all built-in extensions
```

### Writing an extension

```typescript
// extensions/git/index.ts
import { lazy } from "react";
import { GitBranch } from "lucide-react";
import { views, commands } from "@/core/studio";

export function activate() {
  views.register("git", {
    title: "Git",
    icon: GitBranch,
    defaultZone: "sidebar",
    component: lazy(() => import("./panel")),
  });

  commands.register("git.commit", {
    title: "Git: Commit",
    handler: async () => { /* ... */ },
  });
}
```

## How it works

**Runtime** owns all data. It manages PostgreSQL (:5480), parses app manifests into tables, and exposes a REST API for CRUD on collections.

**Studio** is the IDE. It talks to the Runtime via HTTP, manages the AI Forge sidecar, and provides an extensible workspace.

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
