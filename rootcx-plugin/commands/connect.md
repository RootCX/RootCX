---
description: Connect the current workspace to a RootCX Core (local or remote). Saves URL, authenticates automatically if the Core requires it (password or OIDC).
argument-hint: <url> [--token <token>]
---

The user wants to connect the current workspace to a RootCX Core.

Run the `rootcx` CLI via Bash:

```
rootcx connect <url> [--token <token>]
```

If the user did not provide a URL, ask for it. Default is `http://localhost:9100` for local Cores.

The CLI handles authentication automatically:
- If the Core has no auth → connects directly
- If first-time setup → prompts for email/password to create admin account
- If password login is enabled → prompts for email/password
- If OIDC is configured → opens browser for SSO login
- If a `--token` is provided → uses it as bearer token (for CI/scripts)

After connecting, confirm the connection to the user.
