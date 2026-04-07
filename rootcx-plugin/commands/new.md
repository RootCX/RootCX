---
description: Scaffold a new RootCX app or AI agent in the current directory.
argument-hint: app|agent <name>
---

The user wants to scaffold a new RootCX project.

1. Ask which kind (`app` or `agent`) and the project name, if not given.
2. Run: `rootcx new <app|agent> <name>`
3. Then help the user fill in the scaffolded files based on what they want to build.

**Crucial**: the scaffold is minimal by design. You are responsible for producing the real code. Use the RootCX skills to stay 100% compliant:

- `rootcx-manifest` — for editing `manifest.json` (entities, fields, RBAC)
- `rootcx-sdk-hooks` — for data fetching in `src/App.tsx` and other components
- `rootcx-ui` — for UI components, layout, dark mode, AuthGate pattern
- `rootcx-backend-worker` — only for regular apps with a `backend/` worker (RPC, jobs, IPC)
- `rootcx-rest-api` — when calling the Core REST API directly
- `rootcx-agent` — for AI agents: `backend/agent.json`, system prompt, LangGraph `index.ts`

Never invent APIs, hooks, or manifest fields that are not documented in these skills.
