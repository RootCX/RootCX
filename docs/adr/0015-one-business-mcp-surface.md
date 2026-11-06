# ADR 0015: One business MCP surface per tenant, with a fixed tool list

Status: accepted 5 November 2026, implemented from `shapp/main` 1577f9c. Not released.

## Context

Two MCP servers answer for the same tenant data.

- Core's `/mcp` (`core/src/mcp_server.rs`) authenticates audience-bound tokens
  with `mcp:read` and `mcp:write` but offers builder tools only:
  `get_project_context`, `get_app`, `validate_manifest`, `create_records`.
- A product gateway rebuilds a business MCP outside Core: one tool per declared
  action, hints guessed from action names, actions withheld by name pattern,
  `mutate_data` split to hide `delete`, actions called through `/apps/{app}/rpc`.

Agents (ADR 0011) use `list_actions` and `call_action`, which require agent
context. The action grains are checked twice and disagree: `rpc_proxy` requires
the fine `app:{app}:action:{id}` grant for an approved action (ADR 0008), while
`CallActionTool` accepts the coarse `app:{app}:invoke` for every action and does
not pose the isolated scope (ADR 0003).

Rules therefore depend on the entry: an agent may send, sign and delete; an MCP
client may not. A per-action tool list also changes with every installed or
custom app, so MCP hosts that review tools before use never see custom apps.

## Decision

Core's `/mcp` is the only business MCP surface of a tenant. Every assistant
(MCP clients and agents) uses the same Modules and rules.

1. **Action invocation Module** (`core/src/governance/action_invocation.rs`).
   `Declaration::load` reads one declared action; `invoke` checks the grains
   (only `app:{app}:action:{id}` for an approved action, `invoke` or that grain
   otherwise), refuses assistants undeclared or `humanOnly` actions, chooses the
   dedicated process (only for `isolatedScope` or approved actions) and
   dispatches through `ActionCaller`. `rpc_proxy`, the agent `call_action` tool
   and the MCP `call_action` tool call it. `permits` gives listings
   (`list_actions`, `list_apps`, `describe_app`) the same decision. This fixes
   two divergences: the agent tool accepted `invoke` for approved actions, and
   started a dedicated process for every action.
2. **Assistant origin.** `ToolContext::is_assistant()` is true for agent runs
   (an `AgentAuthority` is present) and for `/mcp` calls (`via_mcp`, set only by
   Core's MCP server). Request bodies cannot set it.
3. **Assistant rules, declared once.** `ActionDefinition.humanOnly` (default
   `false`) keeps an action from every assistant and out of their listings.
   `mutate_data` refuses `delete` to every assistant. No name pattern decides.
4. **Scopes follow declarations.** On `/mcp`, `call_action` needs only
   `mcp:read` for an action declaring `readOnlyHint: true` that is not approved
   (approved actions carry data authority); anything else needs `mcp:write`.
   Grants are checked before scopes, so a caller without them learns nothing
   about the app's declarations. Results reach MCP clients without internal
   identities (`created_by`, `owner_id`…). Undeclared actions count as writes.
5. **Fixed `/mcp` tool list:** `get_project_context`, `describe_app`,
   `query_data`, `create_record`, `update_record`, `call_action`,
   `upload_file`, `validate_manifest`. No argument selects an operation (an
   OpenAI review rule): `create_record` and `update_record` each run
   `mutate_data` with a fixed operation, and MCP offers no deletion.
   `describe_app` and `query_data` are served from the tool registry with their own descriptors and hints, not
   re-implemented; `call_action` and `upload_file` are MCP tools over the
   shared Modules. `get_app` and `create_records` are removed: `describe_app`
   and `create_record` cover them with the caller's permissions, where `get_app`
   returned the full contract to any signed-in user. `upload_file` checks the
   storage grant, then stores through the same function as the storage upload
   route; it also accepts a file passed by reference (`openai/fileParams`),
   fetched only from HTTPS hosts listed in `ROOTCX_MCP_FILE_HOSTS`, every
   redirect checked.
6. **Product gateways stay outside Core.** A gateway authenticates its accounts,
   obtains an `/mcp` audience token through the existing token exchange with
   the scopes the person granted, and forwards messages. Core holds no product
   name or case.

## Compatibility

`list_actions`, agent `call_action`, `/apps/{app}/rpc`, `get_project_context`
and `validate_manifest` keep their names and inputs. MCP clients lose `get_app`
and `create_records` (use `describe_app` and `create_record`).
Behaviour changes on purpose:
- `call_action` now requires the fine grant for approved actions, as `rpc_proxy`
  already did (ADR 0008).
- Agents can no longer delete records or run `humanOnly` actions.
- `list_actions` lists only the actions the caller may run, as the other
  listings already did; agents share their worker unless an action asks for
  its own process.
- Product gateways must deploy after Core: they rely on its fixed tool list.
Manifests without `humanOnly` or annotations keep today's behaviour, except that
an assistant on `mcp:read` cannot run an undeclared action.

## Validation

Real HTTP, worker and PostgreSQL tests through the three callers of the action
invocation Module: grains (coarse, fine, approved), isolated scope, `humanOnly`,
assistant delete, `mcp:read` versus `mcp:write`. Existing isolation check: no product name in Core.
