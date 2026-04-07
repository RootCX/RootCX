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
  <img src="https://rootcx.com/docs/cloud/crm-desktop.png" alt="RootCX - CRM running on RootCX Cloud" width="800" />
</p>

<br />

## Table of Contents

- [What is RootCX?](#what-is-rootcx)
- [Features](#features)
- [Quickstart](#quickstart)
- [Architecture](#architecture)
- [Development](#development)
- [Community](#community)
- [License](#license)

## What is RootCX?

RootCX is an open-source\* infrastructure for building custom internal software and AI agents. Build a unified fleet of interconnected apps that combines the experience of a modern SaaS with the robustness of an ERP. Full ownership of your code, absolute control over your data.

**Develop locally. Deploy anywhere. Self-host or use our cloud.**

## Features

- **Fleet of apps** sharing one database, one auth system, one permission model
- **AI Agents** with built-in tools, session memory, supervision policies, and cross-agent delegation
- **OIDC Single Sign-On** -- Azure AD, Okta, Google Workspace, Auth0
- **Global RBAC** with namespaced permissions (`app:crm:contacts.read`) and wildcard matching
- **Channels** -- connect agents to Telegram in one click
- **Integrations** -- Apollo, GitHub, Stripe, Slack, and custom connectors
- **Automatic schema sync** -- define your data model, Core creates and migrates the database
- **Immutable audit log** -- every INSERT, UPDATE, DELETE captured at the database trigger level
- **AES-256 encrypted secret vault** -- API keys and credentials, never stored in plaintext
- **Build from anywhere** -- Studio (desktop IDE), CLI, or Claude Code

<p align="center">
  <img src="https://rootcx.com/docs/cloud/fleet-3d.png" alt="Fleet 3D view with AI Agent" width="800" />
</p>

## Quickstart

### Option A: RootCX Cloud (fastest)

No installation, no Docker, no infrastructure. A managed Core is provisioned for you in minutes.

1. Sign up at [rootcx.com/app/register](https://rootcx.com/app/register).
2. Create a project and hit **Launch Project**.
3. Once active, copy the **API URL** from the project dashboard.

### Option B: Run locally

**Prerequisite:** [Docker Desktop](https://docker.com/get-started) must be installed.

```bash
git clone https://github.com/rootcx/rootcx.git && cd rootcx
docker compose up -d
```

Core is running at `http://localhost:9100`.

### Connect and build

Once you have a running Core, choose your tool:

**Studio** (desktop IDE):
1. [Download Studio](#download-studio) and open it.
2. Select **Connect to a server** and paste your Core URL.
3. Open AI Forge, describe what you want, hit Run (F5).

**CLI**:
```bash
rootcx connect http://localhost:9100
rootcx new agent support_bot
# ... build your agent ...
rootcx deploy
```

**Claude Code**:
```bash
/rootcx-connect http://localhost:9100
/rootcx-new agent support_bot
# Claude Code builds it using 6 official RootCX skills
/rootcx-deploy
```

See the [Getting Started guide](https://rootcx.com/docs/guides/getting-started) for a full walkthrough.

### Download Studio

| Platform | Download |
|----------|----------|
| macOS (Apple Silicon) | [RootCX Studio (.dmg)](https://github.com/RootCX/RootCX/releases/latest/download/RootCX.Studio_aarch64.dmg) |
| macOS (Intel) | [RootCX Studio (.dmg)](https://github.com/RootCX/RootCX/releases/latest/download/RootCX.Studio_x86_64.dmg) |
| Windows | [RootCX Studio (.exe)](https://github.com/RootCX/RootCX/releases/latest/download/RootCX.Studio_x64-setup.exe) |
| Linux (.deb) | [RootCX Studio (.deb)](https://github.com/RootCX/RootCX/releases/latest/download/RootCX.Studio_amd64.deb) |
| Linux (.AppImage) | [RootCX Studio (.AppImage)](https://github.com/RootCX/RootCX/releases/latest/download/RootCX.Studio_amd64.AppImage) |

## Architecture

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset=".github/architecture.svg" />
    <source media="(prefers-color-scheme: light)" srcset=".github/architecture.svg" />
    <img src=".github/architecture.svg" alt="RootCX Architecture" width="800" />
  </picture>
</p>

**Core** is a Rust daemon that powers your entire fleet. Every app and agent you deploy inherits the same enterprise primitives:

- PostgreSQL with automatic schema sync -- no migration files
- Automatic CRUD APIs generated from your data model
- JWT authentication with OIDC SSO (Azure AD, Okta, Google)
- Global RBAC with namespaced permissions, inheritance, and wildcards
- Immutable audit logs at the database trigger level
- AES-256 encrypted secret vault
- Isolated Bun process supervisor with crash recovery
- Durable background job queue with automatic retry
- Real-time log streaming via SSE
- Channels for connecting AI agents to messaging platforms (Telegram)

<p align="center">
  <img src="https://rootcx.com/docs/cloud/rbac-roles.png" alt="RBAC with namespaced permissions" width="800" />
</p>

**Studio** is a native desktop IDE built with Tauri. Build apps, AI agents, integrations, and MCP servers. Deploy with a single keystroke.

- AI Forge: describe intent in plain language, get production-ready code
- Visual database browser and SQL editor
- Governance UI for RBAC, audit logs, secrets, and auth
- Integration catalog with one-click connect
- Live log streaming and process monitoring

<p align="center">
  <img src="https://rootcx.com/docs/cloud/tools-catalog.png" alt="Integration and tools catalog" width="800" />
</p>

**CLI + Claude Code** for developers who prefer the terminal:

- `rootcx` CLI for scaffolding, deploying, and invoking agents
- Claude Code plugin with 6 official skills for AI-assisted development
- Same output as Studio -- fully compatible, switch tools anytime

<p align="center">
  <img src="https://rootcx.com/docs/cloud/database-browser.png" alt="Database browser" width="400" />
  <img src="https://rootcx.com/docs/cloud/sql-editor.png" alt="SQL editor" width="400" />
</p>

<p align="center">
  <img src="https://rootcx.com/docs/cloud/audit-log.png" alt="Audit log with before/after diff" width="400" />
  <img src="https://rootcx.com/docs/cloud/secrets-vault.png" alt="Encrypted secret vault" width="400" />
</p>

## Development

```bash
# Clone the repo
git clone https://github.com/rootcx/rootcx.git && cd rootcx

# Download Bun runtime
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
