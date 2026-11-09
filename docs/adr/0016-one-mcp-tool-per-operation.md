# ADR 0016: One MCP tool per operation, generated from installed apps

Status: accepted 9 November 2026. Supersedes decision 5 and the MCP half of
decision 2 of ADR 0015.

## Context

ADR 0015 gave `/mcp` a fixed tool list: `call_action` ran any declared action
named by its arguments, `describe_app` returned the actions and entities with
their schemas, and `query_data`, `create_record` and `update_record` worked on
any entity named by an argument. Directory review flags `call_action` and
`describe_app` as blocking: "This tool appears to execute operations that are
not individually exposed. Please expose each model-callable operation upfront
as a separate tool with a clear description, input schema, and annotations."
The rule reads "Do not use discovery, operation selection, or schema fetching
with a generic executor" (https://developers.openai.com/plugins/plugin-guidelines).
The record tools needed `describe_app` for their schemas.

The MCP specification lets the tool list vary by the authorization presented
(https://modelcontextprotocol.io/specification/latest/server/tools).

## Decision

1. **One Module generates and resolves every app tool**
   (`core/src/mcp_app_tools.rs`). From the installed manifests and the
   caller's permissions it builds:
   - one tool per action an assistant may run, `<app id>__<action id>`: title
     from `name`, description `<app name>: <description>`, `inputSchema`,
     `outputSchema` under `result`, declared hints. Undeclared hints take the
     worst case; an approved action is never read-only;
   - one read-only tool per entity the caller may read, `<app id>__<entity>__list`:
     exact-match filters on the entity's comparable fields (types and enums from
     the data contract), `orderBy` limited to its readable fields, `order`,
     `limit` (100 at most), `offset`. Hints: read-only, not destructive,
     idempotent, closed world; scope `mcp:read`.
2. **Listing and calling share one decision.** A call resolves its name through
   the same generation as the list, so a tool is callable exactly when it is
   listed. A name two operations would share is offered for neither.
3. **No second rule path.** Actions: `list_actions::invocable` (which applies
   `action_invocation::authorize`) decides, then the scope check and
   `action_invocation::dispatch` run them. Reads: listed when the caller holds
   `tool:query_data` and may read the entity (`.read`, `.read.own` or
   `.read.shared`), and run the registry's `query_data` with the person's
   permissions, so RLS, column privileges and audit apply as on every entry.
   Sensitive fields are neither filters nor sorts and are never returned.
   Derived entities (ADR 0012) get no tool. A filter takes one value: no
   operator or raw `where` reaches the query. Integrations get no tools: apps
   reach them through their bindings. An action whose input schema cannot
   stand alone (root not `type: object`, or a `$ref` resolving outside the
   schema) gets no tool: a host refuses the whole list over one invalid schema
   (ChatGPT: "Invalid MCP tool schema for tool 'gmail__create_draft'").
4. **Records change only through actions.** MCP offers no generic create,
   update or delete: actions apply the app's rules (numbering, versions,
   totals). `call_action`, `describe_app`, `query_data`, `create_record` and
   `update_record` leave MCP; agents keep the registry tools.
5. **`/mcp` keeps three workspace tools**: `get_project_context` (now with each
   app's `instructions`), `upload_file`, `validate_manifest`.
6. **The list depends on the caller**: installed apps and grants. It is
   returned with `cacheScope: private`.
7. `ToolContext::via_mcp` is removed: MCP no longer runs registry tools as an
   assistant; the read tool fixes its own operation.

## Consequences

- Apps write no MCP code: actions and the data contract are the whole input.
- A host that publishes the tools it reviewed shows those of the apps installed
  in the reviewed workspace. Others are reachable through hosts that read the
  live list (direct connections, Claude).
- An app changing an action's id or inputs, or an entity, changes its tool.
  Apps keep them compatible and add a new action for a breaking change.
- Every readable entity gets a read tool, including entities an app keeps only
  for old data.
- Agents, the RPC route and the governance rules are unchanged.

## Validation

`mcp_app_tools` unit tests: action and read definitions from declarations
(types, enums, sensitive, paging names), tools of one app by permissions
(humanOnly, unreadable, derived, missing grants), name collisions, invalid
names, list inputs (no operator, no raw where). Real HTTP tests in
`api_integration.rs`: the list holds one tool per allowed action and readable
entity; a call runs the action; people-only, undeclared and uninstalled tools
are refused; `mcp:read` runs a read and a declared read-only action and refuses
a write; a `.read.own` reader sees only its rows and no tool for an unreadable
entity.
