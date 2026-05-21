<p align="center">
  <a href="https://rootcx.com">
    <img src="https://rootcx.com/logo.svg" width="60" />
  </a>
</p>

<h1 align="center">RootCX</h1>

<p align="center">
  <strong>Build internal tools with Claude Code. Ship them with enterprise governance.</strong><br />
  One server gives you database, auth, RBAC, audit logs, secrets, and deployment<br />
  for every internal tool you build with AI.
</p>

<p align="center">
  <a href="https://rootcx.com/docs"><img src="https://img.shields.io/badge/docs-rootcx.com-blue" alt="Documentation" /></a>
  <a href="https://discord.gg/W7sqMYtdws"><img src="https://img.shields.io/discord/1472936179383930950?color=5865F2&label=Discord&logo=discord&logoColor=white" alt="Discord" /></a>
  <a href="https://github.com/rootcx/rootcx/blob/main/LICENSE.md"><img src="https://img.shields.io/badge/license-FSL--1.1--ALv2-blue" alt="License" /></a>
</p>

<br />

## The problem

You build an internal tool with Claude Code, Codex, Cursor, or your favorite AI coding tool. The app takes 5 minutes. Then you're stuck figuring out:

- Where does it run?
- Who can access what?
- Where's the database?
- Is there an audit trail?
- How do I manage secrets, cron jobs, integrations?

Every tool you build becomes its own infrastructure project. No central place, no governance, no way to manage them all.

## How it works

You keep coding exactly like you do today. Same AI tools, same freedom. RootCX is just the production layer underneath.

Deploy Core once (one Docker image or [Cloud](https://rootcx.com)). Every internal tool you build lands on the same governed platform with everything already wired up.

```bash
rootcx init
```

<p align="center">
  <img src=".github/demo.gif" alt="rootcx init demo" width="800" />
</p>

From zero to deployed in one command. Pick cloud or self-hosted, name your app, scaffold, deploy.

## What every app gets for free

**Database** - Shared PostgreSQL. Define a schema, get CRUD APIs automatically. No ORM, no migrations.

**Enterprise auth** - SSO (Okta, Entra ID, Google Workspace, Auth0). Namespaced RBAC with role inheritance. One line to protect a route.

**Governance** - Full audit log (every mutation, who did what, before/after diff). Encrypted secrets vault. Centralized visibility across all your internal tools.

**Infrastructure** - Cron scheduling, durable message queues, file storage, 20+ integrations (Slack, GitHub, Salesforce, Stripe...).

## Get started

### Install the CLI

```sh
# macOS / Linux
curl -fsSL https://rootcx.com/install.sh | sh

# Windows
powershell -c "irm https://rootcx.com/install.ps1 | iex"
```

Then scaffold and deploy your first app:

```bash
rootcx init
```

### Install the skill (Claude Code)

```bash
npx skills add rootcx/skills
```

Works with Claude Code, Codex, Cursor, or your favorite AI coding tool. You don't change how you build. RootCX is where your code lands.

## How it compares

| | RootCX | DIY (Supabase + Auth0 + ...) | Retool / Airplane |
|---|---|---|---|
| You code with AI | First-class (Claude Code, Codex, Cursor, any) | Not designed for it | Proprietary builder |
| Governance | RBAC + audit log + secrets vault | Build it yourself | Partial |
| Setup | One server, one command | 5+ services to glue | Hosted only |
| Self-hosted | Single Docker image | Complex | Enterprise only |
| You own the code | Yes | Yes | No |
| All your tools in one place | Yes | Scattered | Yes |

## Architecture

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset=".github/architecture.svg" />
    <source media="(prefers-color-scheme: light)" srcset=".github/architecture.svg" />
    <img src=".github/architecture.svg" alt="RootCX Architecture" width="800" />
  </picture>
</p>

## All features

| | |
|---|---|
| **Database** | Shared PostgreSQL with auto-generated CRUD APIs |
| **Auth** | OIDC SSO (Okta, Microsoft Entra ID, Google Workspace, Auth0) |
| **RBAC** | Namespaced permissions, wildcard matching, role inheritance |
| **Audit log** | Every mutation captured with before/after diff |
| **Scheduled jobs** | Cron scheduling via `pg_cron` |
| **Message queue** | Durable job queue via `pgmq` with automatic retry |
| **Secrets vault** | AES-256 encrypted storage for API keys and credentials |
| **Integrations** | Notion, Gmail, Outlook, Salesforce, Slack, GitHub, Stripe, and more |
| **Agent tools** | Every app exposes tools (query, mutate, describe) for agents |
| **MCP** | Connect any MCP server to give agents access to external tools |
| **Channels** | Connect agents to Telegram, Slack, email |
| **File storage** | Upload and serve files scoped per app |

## Community

- [Discord](https://discord.gg/W7sqMYtdws) - Questions, discussion, support
- [GitHub Issues](https://github.com/rootcx/rootcx/issues) - Bug reports and feature requests
- [Docs](https://rootcx.com/docs) - Guides, references, API docs

## License

Source-available under [FSL-1.1-ALv2](LICENSE.md). Use, modify, and redistribute for any purpose other than offering a competing product. Converts to **Apache 2.0** after two years.
