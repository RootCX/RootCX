<p align="center">
  <a href="https://rootcx.com">
    <img src="https://rootcx.com/logo.svg" width="60" />
  </a>
</p>

<h3 align="center">The secure open-source* foundation for internal software and AI agents</h3>

<p align="center">
Build a fleet of interconnected apps and AI agents with managed database, Auth, RBAC, and an AI desktop studio out of the box.<br/>Fully managed, or self-hosted for absolute data sovereignty.
</p>

<p align="center">
  <a href="https://rootcx.com/docs"><img src="https://img.shields.io/badge/docs-rootcx.com-blue" alt="Documentation" /></a>
  <a href="https://discord.gg/W7sqMYtdws"><img src="https://img.shields.io/discord/1472936179383930950?color=5865F2&label=Discord&logo=discord&logoColor=white" alt="Discord" /></a>
  <a href="https://github.com/rootcx/rootcx/blob/main/LICENSE.md"><img src="https://img.shields.io/badge/license-FSL--1.1--ALv2-blue" alt="License" /></a>
  <a href="https://github.com/rootcx/rootcx/stargazers"><img src="https://img.shields.io/github/stars/rootcx/rootcx?style=social" alt="Stars" /></a>
</p>

<p align="center">
  <a href="https://rootcx.com">Website</a> · <a href="https://rootcx.com/docs">Docs</a> · <a href="https://discord.gg/rootcx">Community</a> · <a href="https://rootcx.com/docs/guides/getting-started">Get Started</a>
</p>

<br />

<p align="center">
  <img src="https://rootcx.com/docs/rootcx-studio.png" alt="RootCX Studio" width="800" />
</p>

<br />

## What is RootCX?

RootCX is an open-source\* infrastructure for building custom internal software and AI agents. Build a unified fleet of interconnected apps that combines the experience of a modern SaaS with the robustness of an ERP. Full ownership of your code, absolute control over your data.

**Develop locally. Deploy anywhere. Self-host or use our cloud.**

### How it works

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset=".github/architecture.svg" />
    <source media="(prefers-color-scheme: light)" srcset=".github/architecture.svg" />
    <img src=".github/architecture.svg" alt="RootCX Architecture" width="800" />
  </picture>
</p>

**Core** is a Rust daemon that powers your entire fleet. Every app and agent you deploy inherits the same enterprise primitives:

- Managed PostgreSQL with automatic schema sync
- Automatic CRUD APIs generated from your data model
- JWT authentication and session management
- Granular role-based access control (RBAC)
- Immutable audit logs at the database trigger level
- AES-256 encrypted secret vault
- Isolated process supervisor with crash recovery
- Background job queue
- Real-time log streaming via SSE

**Studio** is a native desktop IDE built with Tauri. You use it to build apps, AI agents, integrations, and MCP servers. Each one is a standalone project with its own code, its own frontend, and its own backend logic. When deployed, they all connect to the same Core and share its database, auth, RBAC, and governance layer.

- AI Forge: describe intent in plain language, get production-ready code
- Visual database browser and schema manager
- Governance UI for RBAC, audit logs, secrets, and auth
- Integrated terminal and live log streaming
- One-click deploy to any connected Core

## Get Started

### Cloud

Sign up at [rootcx.com](https://rootcx.com/app/register), create a project, and connect Studio. Your project gets its own dedicated Core with a database, API, and runtime.

### Development

```bash
# Clone the repo
git clone https://github.com/rootcx/rootcx.git && cd rootcx

# Download bundled PostgreSQL + Bun
make deps

# Start Studio in dev mode (hot reload)
make dev
```

**Prerequisites:** Rust (latest stable), Node.js 18+, pnpm

## Community

- [Discord](https://discord.gg/rootcx) for questions, discussion, and support
- [GitHub Issues](https://github.com/rootcx/rootcx/issues) for bug reports and feature requests
- [Documentation](https://rootcx.com/docs) for guides, references, and API docs

## License

RootCX is licensed under the [FSL-1.1-ALv2](LICENSE.md) (Functional Source License). You can use, modify, and redistribute the software for any purpose other than offering a competing product. The license automatically converts to **Apache 2.0** after two years.

We chose FSL because it lets us build in the open. You get the full source, you can self-host, you can extend it, while protecting the project's ability to sustain itself. After two years, every release becomes fully permissive under Apache 2.0, no strings attached.

---

\* RootCX is source-available under the [FSL-1.1-ALv2](https://fsl.software/), which converts to Apache 2.0 after two years.
