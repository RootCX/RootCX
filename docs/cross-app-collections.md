# Governed cross-application CRUD

An application accesses a provider collection through a Core-approved grant.
The grant names both installation generations, one collection, its allowed
operations, its readable response fields, and its writable fields. Installing
two apps never creates a grant.

## Terms

| Term | Meaning |
| --- | --- |
| Consumer application | The application Core identifies as requesting access. A user-supplied app ID does not establish this identity. |
| Provider application | The application whose collection, schema and row policies govern the data. |
| Installation generation | An installed governance contract. Rotation or uninstall invalidates grants bound to it. |
| Collection grant | Approved operations and separate readable/writable field scopes between consumer and provider installations. |
| Responsible human | The principal whose row access constrains delegated execution; distinct from the agent or application recorded as the audit actor. |
| Job envelope | Core-owned routing and authority around an application's job payload. |

The design rationale is recorded in [ADR 0004](adr/0004-cross-app-collection-grants.md).

Anonymous callers need an additional provider-approved [publication](publications.md).
Core runs declared public reads with a managed server identity; an existing
cross-app grant alone never makes its data public.

## Choose the interaction

| Intent | API | Authority |
| --- | --- | --- |
| Query or change provider records | Worker `ctx.remote(app).collection(entity)`; agent/workflow `query_data` or `mutate_data` | Approved collection grant for the consumer installation, exact operation and fields, plus applicable RLS/delegated permissions |
| Execute provider business logic | Agent tool `call_action` | Tool permission and target action/invoke permission; target must declare the action |
| Delegate a task to another agent | Agent tool `invoke_agent` | Tool permission and target invoke permission; child authority narrows against the parent |
| Call an integration with user credentials | Worker `ctx.callIntegration(...)` | Integration binding and the integration's governed action path |

Collection grants do not authorize action calls or agent invocation. Conversely,
permission to invoke an action or agent does not grant access to its collections.
Target code runs with its own app context and the delegated authority Core passes
to it; a further collection hop needs a grant for that actual consumer app.
There is no `ctx.remote(...).action(...)` or `ctx.invokeAgent(...)`.
`ctx.action(name, input)` calls an action of the current app.

## Approve a relationship

A principal with `admin:cross_app.grants.manage` creates a pending grant:

```http
POST /api/v1/cross-app/grants
Authorization: Bearer <access-token>
Content-Type: application/json

{
  "consumerApp": "sales",
  "providerApp": "crm",
  "entity": "contacts",
  "actions": ["list", "read", "create", "update", "delete"],
  "fields": ["name", "email"],
  "writeFields": ["name", "email"],
  "reason": "Maintain customer contacts from sales"
}
```

Only request the operations the consumer needs. `writeFields` is required for
`create` or `update`; fields are never inferred from future schema changes.
Sensitive fields and system-managed fields cannot be included in writable
scope. Safe system identifiers and timestamps are included in the response
snapshot.

Readable and writable scopes are independent: a field in `writeFields` need
not appear in `fields`, and writing it does not make it visible in the response.
Omitting `fields` freezes the current nonsensitive fields; prefer an explicit
list for a narrow contract. An explicit `fields` list must be nonempty;
`fields: ["id"]` requests only safe system fields.
Read/delete-only grants should omit `writeFields` or set it to `[]`.
Both apps must have active installation generations before requesting a grant.

Approve the returned grant ID with `admin:cross_app.grants.approve` or the
provider-specific `app:crm:cross_app.approve` permission:

```http
POST /api/v1/cross-app/grants/<grant-id>/approve
Authorization: Bearer <approver-access-token>
Content-Type: application/json

{"reason": "Approved by the data provider"}
```

`GET /api/v1/cross-app/grants` lists grants. Provider-scoped administrators
supply `?providerApp=crm`. The `/disable`, `/enable`, and `/revoke` operations
take the same reason body. Revoked and expired grants cannot be re-enabled.

## Worker API

Protocol v5 workers use the injected context:

```typescript
serve({
  rpc: {
    async createContact(params, _caller, ctx) {
      return ctx.remote("crm").collection("contacts").create({
        name: params.name,
        email: params.email,
      });
    },
  },
});
```

The collection supports:

```typescript
const contacts = ctx.remote("crm").collection("contacts");
const matching = await contacts.find({ name: "Ada" }); // projected records, no implicit limit
const page = await contacts.findPage({
  where: { name: { $ilike: "Ada%" } },
  orderBy: "name",
  order: "asc",
  limit: 25,
  offset: 0,
}); // { data, total }; total remains correct beyond the last page

const contact = await contacts.findOne({ id }); // projected record or null
const created = await contacts.create({ name, email }); // insert is an alias
const updated = await contacts.update({ id, email }); // update(id, { email }) also works
const deleted = await contacts.delete(id); // { id, deleted: true }
```

| Worker operation | Required grant action | Result |
| --- | --- | --- |
| `find(where?)` | `list` | Array of all visible matching records, without an implicit limit |
| `findPage(options?)` | `list` | `{ data, total }`; explicit pagination |
| `findOne(where?)` | `read` | Projected record or `null` |
| `create(data)` / `insert(data)` | `create` | Projected created record |
| `update({ id, ...data })` / `update(id, data)` | `update` | Projected updated record |
| `delete(id)` | `delete` | `{ id, deleted: true }` |

`list` and `read` are distinct grant actions. Both correspond to provider
`.read` permissions in a delegated execution. Mutations correspond to
`.create`, `.update`, or `.delete`; approving `update` does not approve `create`.

`find` and `findOne` take equality maps. `findPage` takes the query
options envelope; put all filters under `where`. Filtering
and sorting also require readable field scope. Updates and deletes require an
explicit UUID, preventing accidental unbounded writes.

For a column named `limit`, `order`, `offset`, `orderBy`, or `where`,
`find({ limit: 10 })` and `findOne({ limit: 10 })` remain equality lookups.
Use `findPage({ where: { limit: 10 } })` for a paginated query.
Page defaults are `limit: 100`, `offset: 0`, and descending creation time.
Limits must be integers from 1 through 1000; offsets must be nonnegative integers.
`findOne` has no ordering option; use a unique key when you need one specific row.

Mutation responses use the approved readable projection. A create-only grant
can return that response without authorizing a separate `find` or `findOne`.
Unknown, sensitive, system-managed, or unapproved writable fields reject the
whole mutation.

In TypeScript, describe the approved projection rather than the full provider
entity. Types are developer assertions, not runtime validation or authority:

```typescript
interface ContactView {
  id: string;
  name: string;
  email: string | null;
}

async function lookupContact(ctx: RootCxCtx, id: string) {
  const contacts = ctx.remote("crm").collection<ContactView>("contacts");
  const contact = await contacts.findOne({ id });
  if (contact === null) return null;
  return contact.email;
}
```

Include `core/resources/integrations/rootcx-worker.d.ts` in your worker's
TypeScript project to resolve `RootCxCtx` and the injected globals. Match field
nullability to the provider manifest. Per-call generics such as
`contacts.find<ContactView>()` are also supported.

Remote operations cannot run inside `ctx.transaction`; each operation owns a
provider transaction. There is no distributed transaction across applications.
Local and remote collections share the same method signatures and result shapes.
Existing same-app `find`, `findOne`, `insert`, and `update({ id, ...data })` calls
remain valid. Both handles expose `findPage`, `create`, `delete`, and the
`update(id, data)` overload. Neither handle exposes `bulk_create`.
`ctx.remote` selects the provider; it does not change the CRUD vocabulary.

The earlier, unreleased remote API returned a page from `find`. Code written
against that draft should use `findPage` for those calls. The existing raw v5
remote IPC `find`/`list` messages keep their page result for wire compatibility.

`ctx.sql` is confined to the worker's Core-bound application. Fully qualified
provider table names and cross-schema joins cannot bypass grants, even when the
caller has provider permissions or platform-admin permissions. Missing app
context fails closed. Authorized remote operations open a provider-context
transaction only after Core approves the grant and exact operation.

Migrate existing cross-app SQL joins to separate granted `ctx.remote` operations
and combine their projected results in application code. Direct human HTTP
collection, `linked` enrichment, and federated reads retain the user's normal
provider permissions. Core marks those generated data transactions before
applying the restricted database role; workers cannot claim that marker.

## Agents and workflows

Agents use `query_data` and `mutate_data` through Core tool dispatch. Raw remote
IPC from an agent worker is refused so it cannot bypass task scope or
supervision.

For example, these are tool names and their argument objects, not worker methods:

```json
[
  {"tool": "query_data", "args": {"app": "crm", "entity": "contacts", "where": {"name": {"$ilike": "Ada%"}}, "limit": 25}},
  {"tool": "mutate_data", "args": {"app": "crm", "entity": "contacts", "action": "update", "id": "<contact-uuid>", "data": {"email": "ada@example.test"}}},
  {"tool": "call_action", "args": {"app": "crm", "action": "qualify_contact", "input": {"id": "<contact-uuid>"}}},
  {"tool": "invoke_agent", "args": {"app_id": "research", "message": "Summarize this customer's public background"}}
]
```

The target argument is `app` for collection tools and `call_action`, but
`app_id` for `invoke_agent`. `query_data` returns an array when no query options
are supplied and `{ data, total }` when any query option is present. Use
`where: {}` to request a page explicitly. Without options, both local and remote
`query_data` retain the complete array result, with no implicit 100-row cap.
Use explicit pagination for large collections to bound response size; unpaginated
reads still obey query timeouts and return an error rather than a truncated result.
Remote `query_data` needs the `list`
grant action, including when filtering by ID.

Every tool requires `tool:<tool-name>`. `call_action` currently accepts the
target's `app:<app>:invoke` permission or the specific
`app:<app>:action:<action>` permission, and returns the action's raw result.
Use `list_actions` to discover declared actions and input schemas.
`invoke_agent` requires `app:<app>:invoke` and returns
`{ "agent": "<app-id>", "response": "..." }`. Neither tool consumes a
collection grant for the call itself.

Native workflows use the same tools. The consumer app is the Core-created
workflow backing app (`wf-<workflow UUID>`). Approve its grants before executing
nodes that target another application. A request body cannot impersonate this
consumer identity.

For workflow tool nodes, put the argument object in node `params`, with
`kind: { "type": "tool", "toolName": "mutate_data" }` (or `query_data`).
`call_action`, `invoke_agent`, and `call_integration` require agent execution
context. They remain in agent tool discovery but are omitted from the workflow
palette and generic HTTP tool list. Workflow create/update rejects graphs
containing these tools (or unknown tools) with HTTP 400 and the node/tool name;
enabling a legacy saved graph checks it too. Generic HTTP execution returns
HTTP 400 with `requires agent execution context` after authentication and tool
permission checks. Collection grants cannot enable these tools in those
executors. `query_data` and `mutate_data` remain available with their existing
grant and permission requirements; worker `ctx.callIntegration` is unaffected.

A grant never widens an agent or workflow's frozen delegated permission
intersection. Tool permission and the provider operation permission must both
survive delegation and task scope. Row ownership remains enforced.
Durable workflow creates retain their deterministic record IDs on retry.
`bulk_create` remains unavailable inside durable workflows; use per-item
creates.

Generic human HTTP tool requests cannot claim application grant authority.
Direct user collection access continues to use that user's own RLS permissions.

## Diagnose a denied operation

Worker methods reject with an `Error`; they do not return an `{ error }`
collection result. `findOne` returning `null` means no row was visible, not that
Core accepted a missing grant. Updates/deletes of invisible or missing rows
reject rather than returning `null`.

| Error or symptom | Check next |
| --- | --- |
| Grant creation: `create/update grants require explicit nonempty writeFields` | Supply the exact payload fields in `writeFields`, independently of `fields`. |
| `cross-app collection access is not authorized` | Check the actual consumer, provider, entity, active grant, expiry, exact action and readable/writable field scopes. A matching permission key alone is insufficient. |
| `cross-app collection is not available` / `is unavailable` | Have an authorized administrator check installed apps, active generations and fresh approvals after governance-contract updates. Public errors intentionally hide target details. |
| `cross-app collection query failed` | Check query envelope shape, UUIDs, value types, required provider fields, constraints and RLS. Core currently uses this same message for mutation execution failures. |
| `cross-app access requires a Core-bound application worker` | Use a Core-dispatched worker or workflow for grant access. Setting `appId` in generic HTTP tool JSON does not establish consumer identity. |
| `agents must use query_data or mutate_data for cross-app access` | Route the agent operation through those tools so delegation, task scope and supervision apply. |
| `ctx.remote cannot be used inside ctx.transaction` (message prefix) | Move the remote operation before/after the same-app transaction; it cannot share that transaction. |
| `bulk_create is not allowed inside a workflow` (message prefix) | Use per-item `create` nodes to preserve retry-safe record IDs. |
| `cross-app audit unavailable` (message prefix) | Ask the operator to inspect audit/database health. A grant change will not repair audit storage. The explicit `mutation rolled back` variant confirms rollback. |

An administrator can inspect `GET /api/v1/cross-app/grants/<grant-id>` for
`fieldSnapshot`, `writeFieldSnapshot`, actions and status, and
`GET /api/v1/cross-app/audit` for operation outcomes and denial categories.
The operation audit requires `admin:cross_app.grants.manage` and accepts
`?grantId=<grant-id>&limit=100`. Its `correlationId` groups operation records; use
consumer, provider, entity, action and time to find the attempt. Execution failures currently expose
the category `query_validation_or_execution`, not the underlying SQL error.
Do not automatically retry creates after a timeout: the caller may have lost
the response after commit. The public remote worker API takes no idempotency key.

## Enforcement and lifecycle

Core derives the consumer from the worker or workflow, authorizes one exact
operation, and rechecks the grant inside the provider transaction. Revocation
uses the same lock: it waits for an already-authorized transaction to finish,
then prevents subsequent transactions from using the old authority.

The provider's RLS policies and ownership resolvers still apply. Scoped update
permissions cannot transfer a record to an unauthorized owner. Existing
unscoped provider permissions retain their established semantics.

Successful writes and their cross-app success audit commit together. An audit
insertion failure rolls the business write back. Read data is returned only
after a durable success audit. Inspect operation history at
`GET /api/v1/cross-app/audit` and grant transitions at
`GET /api/v1/cross-app/grants/<grant-id>/audit`.

Changes to the governance contract revoke the previous generation before exposing
the new contract. Changes confined to top-level `name`, `version`, `description`,
and `icon` retain existing grants. Data contracts, permissions and other
execution or access configuration remain part of the comparison.
A failed contract update leaves cross-app authority revoked; complete the
installation and obtain fresh approvals. Uninstall/reinstall never revives old
grants. Workflow deletion and revocation commit together.

## Migration

Existing agents and workflows that used foreign collection tools must receive
explicit grants. No migration automatically converts user permissions into
application authority.

### Queue upgrade and delegated continuations

See [ADR 0005: Core-owned job provenance and delegated authority](adr/0005-governed-job-provenance.md)
for the queue contract and migration decision. Upgrade all Core dispatchers
sharing a queue together; do not mix old and new dispatchers during this transition.

Core stamps queue envelopes with their native kind; application payload keys
such as `action_type`, `_hook`, `manual`, and `cron_id` cannot select a native
executor. Manual workflow enqueue atomically binds the queue ID to the execution,
backing app, and responsible user. Dispatch verifies that binding before changing
execution status.

Queued worker continuations retain their frozen delegated ceiling, credential
pin, and audit attribution. Dispatch intersects that ceiling with current human
permissions and, when Core recorded a distinct authority principal, that
principal's current permissions. Disabled principals and revoked recorded
delegations are denied. Raw agent `enqueueJob` is denied because its IPC envelope
has no invocation ID with which Core could bind the invocation's task scope.
Ordinary worker jobs, HTTP jobs, and Core-produced cron/hook/workflow events remain
supported.

Existing queue envelopes without a Core-owned `kind` are quarantined in
`jobs_dlq`, with the original envelope and a reason retained. Core does **not**
reconstruct authority from legacy payloads, including apparently native ones.
Related legacy workflow execution rows may remain queued: an unverified payload
does not authorize marking a row failed. Operators must reconcile those rows
against trusted execution records, cancel obsolete runs, and submit any required
replacement through the authorized workflow or scheduling API. Do not replay an
old envelope by adding a kind or authority field.
