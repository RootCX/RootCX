# ADR 0016: `/mcp` mirrors the Core tools and gives each action its own tool

Status: accepted 9 November 2026. Supersedes decision 5 and the listing part
of ADR 0015.

## Context

ADR 0015 gave `/mcp` a fixed list chosen for MCP: `describe_app`,
`query_data`, `create_record`, `update_record`, `call_action`, beside the MCP
tools `get_project_context`, `upload_file` and `validate_manifest`. Two of them
were MCP-only wrappers (`create_record`, `update_record` over `mutate_data`),
and actions had no tool of their own. MCP is how any assistant reaches a
tenant (Claude, Cursor, Grok, ChatGPT and others), so it must stay a thin,
generic view of what Core already does, with no rule specific to an assistant.

## Decision

1. **`/mcp` offers the Core tools a person may call** outside agent execution,
   exactly as the tool registry declares them (`ToolRegistry::http_descriptors`,
   the list the HTTP tool route already serves, for example `describe_app`,
   `query_data`, `mutate_data`), each listed only when the
   person holds `tool:<name>`. They run through `tools::routes::execute_as_person`
   with `via_mcp`, so the assistant rules apply as for agents: no deletion,
   `humanOnly` actions hidden and refused. Tools that need agent execution
   (`call_action`, `call_integration`, `delete_app`, `execute_sandbox`,
   `invoke_agent`, `reply_file`) stay with agents.
2. **Each action an assistant may run is its own tool**, `<app id>__<action id>`,
   for every installed app, integrations included. It is built from the
   manifest (title from `name`, description `<app name>: <description>`,
   `inputSchema`, `outputSchema` under `result`, declared hints; undeclared
   hints take the worst case; an approved action is never read-only) and runs
   through `action_invocation`, as from the RPC route and the agent
   `call_action` tool. What an assistant must not run is declared `humanOnly`
   in the manifest, never decided in Core.
3. **One decision for listing and calling** (`core/src/mcp_tools.rs`): a call
   resolves its name through the same list, so a tool is callable exactly when
   it is listed; a name two operations would share is offered for neither.
4. **A schema that cannot stand alone gives no tool**: the root must be
   `type: object` and every `$ref` must resolve inside the schema. One such
   declaration otherwise makes a host refuse the whole list (ChatGPT: "Invalid
   MCP tool schema for tool 'gmail__create_draft'", whose schema is a `$ref`
   into its manifest).
5. **Scopes follow hints**: `mcp:read` for a read-only tool, `mcp:write` too
   otherwise. The list depends on the caller and is returned with
   `cacheScope: private`. `get_project_context` gives each app's
   `instructions`.

## Consequences

- MCP adds no rule of its own: Core tools, actions and their governance are
  the same as for WhatsApp and Telegram agents.
- `create_record` and `update_record` are gone; `mutate_data` is offered as the
  registry declares it.
- Hosts with their own review rules (ChatGPT rejects generic executors such as
  `mutate_data`) are handled outside this view, not by special cases here.

## Validation

`mcp_tools` unit tests (action and Core tool definitions, hints and scopes,
standalone schemas, name collisions, tool names). Real HTTP tests in
`api_integration.rs`: the list holds every Core tool a person may call and one
tool per allowed action, integrations included, none for `humanOnly`; actions
and Core tools run; deletion by an assistant, people-only, undeclared and
uninstalled tools are refused; `mcp:read` runs reads and refuses writes.
