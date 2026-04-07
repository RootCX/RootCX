# RootCX — Claude Code plugin

Build and deploy RootCX apps and AI agents directly from Claude Code.

## What this is

An alternative to the RootCX Studio IDE. Claude Code becomes the assistant (replacing Forge), guided by the 6 official RootCX skills, and deploys to any Core (local daemon or remote) through the `rootcx` CLI.

## Prerequisites

- `rootcx` CLI on `PATH` — build from the workspace: `cargo build --release -p rootcx-cli` → binary at `target/release/rootcx`
- A reachable RootCX Core (default `http://localhost:9100`)

## Commands

| Command | Effect |
|---|---|
| `/rootcx-connect <url> [--token]` | Write `.rootcx/config.json` in the cwd and ping the Core |
| `/rootcx-new app\|agent <name>` | Scaffold a minimal app or agent |
| `/rootcx-deploy` | Install manifest, upload backend + frontend, start worker |

## Skills loaded

- `rootcx-manifest` — data contract, entities, RBAC
- `rootcx-sdk-hooks` — `@rootcx/sdk` React hooks
- `rootcx-ui` — `@rootcx/ui` components, AuthGate pattern
- `rootcx-backend-worker` — Bun worker IPC
- `rootcx-rest-api` — Core REST API
- `rootcx-agent` — LangGraph agents

These skills are the single source of truth shared with Forge — code produced by Claude Code through this plugin is 100% compliant with Forge's instructions.

## End-to-end flow

```
/rootcx-connect http://localhost:9100
/rootcx-new agent support-bot
cd support-bot
# describe the agent in natural language, CC fills the files
/rootcx-deploy
rootcx invoke support-bot "hello"
```
