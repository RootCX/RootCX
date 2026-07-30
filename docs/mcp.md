# RootCX MCP server

Each RootCX Core exposes its tenant-native MCP endpoint at:

```text
https://<tenant>/mcp
```

The public RootCX plugin connects through the universal gateway:

```text
https://rootcx.com/mcp
```

The gateway completes OAuth, binds the access token to the workspace selected by
the user, checks OAuth scopes and live membership, and forwards the MCP request to
that workspace's tenant-native Core endpoint. Core remains the authorization seam;
the gateway does not reimplement app or data operations.

It uses MCP Streamable HTTP through the official Rust SDK (`rmcp`). Every call
is attributed to the authenticated RootCX user and uses the existing RBAC and
RLS implementation. Data mutations use RootCX's existing audit trail.

The public gateway supports OAuth authorization-code + PKCE discovery, dynamic
client registration, workspace consent, refresh-token rotation, and revocation.
It exchanges its short-lived, workspace-bound token for a short-lived,
tenant-native Core token with an explicit MCP audience. Core also exposes OAuth
Protected Resource Metadata for direct clients. Local HTTP development keeps
bearer-token compatibility.

## V1 scope

| Tool | Purpose | Existing RootCX module reused |
| --- | --- | --- |
| `get_project_context` | Read workspace URL, status, user permissions, installed apps, and onboarding state | status, identity, RBAC, app list |
| `get_app` | Read one app and its data contract | app describe |
| `validate_manifest` | Validate a manifest and schema drift without mutation | manifest validator, schema verifier |
| `create_records` | Create up to 1,000 user-approved records | bulk CRUD, RLS, audit |

MCP never accepts application source code and never builds or deploys an app.
The RootCX skill uses the local CLI for those operations:

1. Call `get_project_context` and use `workspace.url` as the CLI login target.
2. Install the RootCX CLI locally when it is missing.
3. Authenticate with `rootcx auth login <workspace-url>`.
4. Scaffold with `rootcx new <app-id>`.
5. Build and test in the user's local workspace.
6. Validate the manifest with `validate_manifest`.
7. Explain the deployment and obtain approval.
8. Deploy with `rootcx deploy`.

The CLI keeps the established deployment sequence: install the manifest, upload
the backend and frontend independently when present, and start the worker. Core
does not compile frontend source received through MCP.

## Configuration

- `ROOTCX_PUBLIC_URL` supplies the public tenant hostname, allowed MCP origin,
  canonical CLI login URL, and absolute application URLs.
- `ROOTCX_MCP_ALLOWED_HOSTS` adds comma-separated hostnames when traffic reaches
  Core through another trusted hostname. Loopback hosts remain enabled for local
  development.
- `ROOTCX_OIDC_ISSUER` identifies the authorization server advertised to MCP
  clients.
- `ROOTCX_MCP_ALLOW_LEGACY_BEARER=true` permits non-audience bearer tokens on an
  HTTPS tenant only as an explicit migration escape hatch. It is disabled by
  default.

The activation contract is exposed at `GET /api/v1/onboarding/status`. Core
records the first successful MCP-scoped OAuth authorization, and the existing
frontend deployment endpoint records the first user-deployed application.
System and catalog apps do not complete this onboarding state.
