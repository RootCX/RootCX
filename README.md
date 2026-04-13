<p align="center">
  <a href="https://rootcx.com">
    <img src="https://rootcx.com/logo.svg" width="60" />
  </a>
</p>

<h3 align="center">Ship internal apps and AI agents to production, fast</h3>

<p align="center">
Get a centralized database, SSO, role-based permissions, audit logs, integrations, and deployment infrastructure out of the box.<br/>Cloud or self-hosted. Your code, your data.
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

## The problem

AI app builders can generate internal software in minutes. Then what?

Where does the database live? Who manages auth? How do you enforce permissions? Where are the audit logs? How does your teammate access the app? How do you push an update without breaking production?

Every AI-generated app needs the same boring infrastructure: a database, SSO, role-based access, secrets management, job scheduling, and a deployment target. Today, you either stitch that together yourself or you don't ship.

**RootCX is that infrastructure.** One server, every app and AI agent you build plugs into it. Database, auth, permissions, audit logs, integrations, deployment -- handled. You focus on what the app does, not where it lives.

## Architecture

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset=".github/architecture.svg" />
    <source media="(prefers-color-scheme: light)" srcset=".github/architecture.svg" />
    <img src=".github/architecture.svg" alt="RootCX Architecture" width="800" />
  </picture>
</p>

## Features

| Feature | Details |
|---------|---------|
| **Database** | Shared PostgreSQL for all apps and agents. Auto-generated CRUD APIs. |
| **Auth** | OIDC Single Sign-On (Okta, Microsoft Entra ID, Google Workspace, Auth0). One login for every app and agent in your fleet. |
| **RBAC** | Namespaced permissions, wildcard matching, role inheritance. Same rules for users and agents. |
| **Audit log** | Database-level audit via PostgreSQL triggers. Every insert, update, and delete captured with before/after JSONB diff. Always on. |
| **Scheduled jobs** | Cron scheduling via `pg_cron`. |
| **Message queue** | Durable job queue via `pgmq` with automatic retry. |
| **Secrets vault** | AES-256 encrypted storage for API keys and credentials. |
| **Integrations** | Connectors for Notion, Gmail, Outlook, Salesforce, Slack, GitHub, Stripe, and more. Custom connectors supported. |
| **Tools** | Every app automatically exposes tools (query, mutate, describe). Agents use them out of the box. |
| **MCP** | Connect any MCP server to give your AI agents access to external tools and data. |
| **Channels** | Connect agents to Telegram, Slack, email. Webhook-based, supports media, debounced message batching. |
| **File storage** | Upload and serve files scoped per app. Nonce-authenticated downloads for workers. |

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

**Studio** (desktop app):
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
