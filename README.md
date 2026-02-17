<p align="center">
  <a href="https://rootcx.com">
    <img src="https://rootcx.com/logo.svg" width="80" />
    <h1 align="center">RootCX</h1>
  </a>
</p>

**Open source studio for developing secure business apps and AI agents.** _Build locally, ship anywhere._

  <a href="https://rootcx.com"><img alt="Website" src="https://img.shields.io/website?url=https%3A%2F%2Frootcx.com&up_message=rootcx.com&up_color=blue"></a>
  <a href="https://github.com/rootcx/rootcx/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-Apache%202-blue" alt="License" /></a>
  <a href="https://discord.gg/rootcx"><img src="https://img.shields.io/discord/1472936179383930950?color=5865F2&label=Discord&logo=discord&logoColor=white" alt="Discord" /></a>
  <a href="https://github.com/rootcx/rootcx/stargazers"><img src="https://img.shields.io/github/stars/rootcx/rootcx?style=social" alt="Stars" /></a>

## Mission

**We believe that the future of business software belongs to open source.**

RootCX is an open-source studio for building, deploying and governing fleets of **custom business apps** and **AI agents**.

## How it Works

We orchestrate the best open-source tools into a single layer to provide a seamless experience for both developers and end-users.

The workflow is simple: **develop locally, then ship anywhere.** You build your apps and AI agents in the Studio on your machine. When you're ready, you can self-host the runtime or use our cloud.

### Architecture

RootCX is built on two core pillars:

1. **RootCX Studio**: A high-performance, extension-first IDE built with **Rust and Tauri v2**.
2. **RootCX Runtime**: A secure-by-design backend daemon that manages the **bundled PostgreSQL** lifecycle. It serves as the fleet's brain, automatically generating APIs, AI tools, and a shared **Governance Layer** (Auth/SSO, RBAC, and Audit Logs) for every app and agent.

```text
    ┌──────────────────┐          ┌──────────────────┐
    │  RootCX Studio   │          │  AI Agents/Apps  │
    │ (Dev Environment)│          │                  |
    └────────┬─────────┘          └────────┬─────────┘
             │                             │
             └────────────┬────────────────┘
                          │
                          ▼
             ┌─────────────────────────────┐
             │       RootCX Runtime        │
             │ (DB, API, Auth, RBAC, Audit)│
             └─────────────────────────────┘
```

## License

RootCX is licensed under [Apache-2.0](LICENSE).

---

<p align="center">Built for the era of auditable, open-source intelligence.</p>
