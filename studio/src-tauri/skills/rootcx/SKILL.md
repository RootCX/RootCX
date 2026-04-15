---
name: rootcx
description: Build internal apps and AI agents on RootCX — the open-source platform with shared PostgreSQL, auto-generated CRUD APIs, OIDC SSO, role-based access control, audit logging, scheduled jobs, message queuing, encrypted secrets, file storage, managed deployment, and pre-built integrations. Get everything you need to ship internal tools to production out of the box.
license: Apache-2.0
compatibility: Requires the RootCX CLI (curl -fsSL https://rootcx.com/install.sh | sh)
metadata:
  version: 0.1.0
---

# RootCX App Development

RootCX is a low-code platform with a Rust/Axum Core, a Studio IDE, and an embedded AI engine. Apps are built with TypeScript/React using `@rootcx/sdk` hooks and `@rootcx/ui` components, backed by a `manifest.json` data contract that Core syncs to PostgreSQL.

## Rules

- [Manifest](./rules/manifest.md) — data contract, entities, field types, schema sync
- [SDK Hooks](./rules/sdk-hooks.md) — useAppCollection, useAppRecord, useIntegration, queries
- [UI & Styling](./rules/ui.md) — components, layout, routing, dark mode, AuthGate
- [UI Components](./rules/ui-components.md) — full component catalogue with prop signatures
- [Backend Worker](./rules/backend-worker.md) — Bun worker IPC protocol, RPC, jobs
- [REST API](./rules/rest-api.md) — Core HTTP API overview, where operators
- [REST API — Collections](./rules/rest-api-collections.md) — CRUD endpoints, query params
- [REST API — Integrations](./rules/rest-api-integrations.md) — bind, actions, auth
- [REST API — Jobs](./rules/rest-api-jobs.md) — async job queue
- [AI Agent](./rules/agent.md) — LangGraph backend, agent.json config, SSE invoke
