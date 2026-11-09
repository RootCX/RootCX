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

## Tools

`/mcp` is a thin, generic view of what Core already does (ADR 0016), the same
for every assistant:

| Tool | Purpose | Scope |
| --- | --- | --- |
| `get_project_context` | Workspace URL, status, user permissions, installed apps with their instructions, onboarding state | `mcp:read` |
| `upload_file` | Store a file in an app, as base64 or as a host file reference | `mcp:write` |
| `validate_manifest` | Validate a manifest and schema drift without mutation | `mcp:read` |
| Core tools | Every tool the registry offers outside agent execution (for example `describe_app`, `query_data`, `mutate_data`, `list_apps`), as it declares itself, listed when the person holds `tool:<name>` | `mcp:read` when declared read-only, else `mcp:write` |
| `<app id>__<action id>` | One declared action of an installed app or integration, run through the action invocation Module. Never listed for `humanOnly` actions; an approved action needs its own grain | `mcp:read` when declared read-only and not approved, else `mcp:write` |

Every call runs under the assistant rules, as for agents: no deletion, no
`humanOnly` action. An action tool carries the action's `name` as title,
`<app name>: <description>` as description, its `inputSchema` (else an empty
object), its `outputSchema` under `result`, and its declared hints; undeclared
hints take the worst case. An action whose input schema cannot stand alone
(root not `type: object`, or a `$ref` that does not resolve inside the schema)
gets no tool, so one bad declaration never makes a host refuse the whole list.
Ids that cannot form a tool name (letters, digits, `_`, `-`, `.`, 128 at most)
are not listed, and a name two operations would share is offered for neither.
The list depends on the caller and is returned with `cacheScope: private`.
Calling a tool the list does not offer returns a tool error.

Results omit internal identities (`created_by`, `owner_id`…).

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
- `ROOTCX_MCP_FILE_HOSTS` lists the HTTPS hosts (and their subdomains) whose
  file links `upload_file` may download. Empty refuses file references.
- `ROOTCX_MCP_ALLOW_LEGACY_BEARER=true` permits non-audience bearer tokens on an
  HTTPS tenant only as an explicit migration escape hatch. It is disabled by
  default.

The activation contract is exposed at `GET /api/v1/onboarding/status`. Core
records the first successful MCP-scoped OAuth authorization, and the existing
frontend deployment endpoint records the first user-deployed application.
System and catalog apps do not complete this onboarding state.
