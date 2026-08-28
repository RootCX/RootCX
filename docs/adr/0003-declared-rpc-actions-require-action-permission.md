# ADR 0003: Declared RPC actions require their action permission

Status: Accepted (2026-08-28)

## Context

Installing a manifest action registers `app:{app}:action:{id}`, and agent
`call_action` already enforces that permission. The authenticated app RPC route,
however, checked only `app:{app}:invoke`. A user could therefore call a declared
action directly without holding the permission that the manifest says governs
it.

Apps also have undeclared RPC methods for internal reads and health checks.
Requiring an action permission for every method would break those methods and
would turn declaration into a prerequisite for ordinary RPC.

Public RPCs use a separate authority model: Core validates the public manifest
declaration, share scope, and read-only delegated permissions before dispatch.
They do not represent an authenticated user exercising an app action.

## Decision

For an authenticated user RPC, Core first requires `app:{app}:invoke`. If the
requested method is the ID of an action in the installed app manifest, Core also
requires `app:{app}:action:{method}` before spawning or dispatching the worker.

Methods absent from `manifest.actions` retain the invoke-only contract. Share
token and anonymous branches retain their existing `manifest.public.rpcs`
authorization and scope checks.

This route guard does not widen SQL permissions. Worker SQL and callback
transactions continue to run under the caller's RLS identity and effective
entity permissions. Core additionally carries a non-forgeable workflow scope
for governed SQL, as described below.

## Consequences

- Direct UI RPC and agent `call_action` agree on the permission attached to a
  declared action.
- Removing an action declaration restores invoke-only behavior for that method;
  action declarations are therefore part of the authorization contract.
- Applications can make workflow-only write invariants structural by checking
  the Core-owned invocation settings from a trigger or policy. Generic CRUD,
  internal RPCs, and public RPCs receive no workflow authority.

## Structural workflow enforcement

An action context must not be added to today's identity-only worker
configuration without also partitioning the worker by action, nor accepted as
an application-supplied SQL field. One identity-bound worker may execute
several RPCs concurrently, so either shortcut would let one invocation borrow
another invocation's action or would make the value directly forgeable.

An invocation ID alone is not a security boundary: app code shares the worker
process with the prelude and can write raw IPC, so it could copy any capability
visible to that process. Core therefore derives authority outside the worker
and isolates it in the same way it isolates user identity. The implementation
uses supervisor partitioning and worker protocol v4:

1. A private Core execution-scope type distinguishes declared actions and
   trusted cron jobs from calls with no workflow authority. The trusted
   route, action callback, or scheduler derives it after authorization;
   neither method parameters, job payloads, nor worker IPC can set it.
2. The worker key and immutable worker configuration include the execution
   scope. A process with no workflow authority can never issue SQL bearing one,
   an action A process cannot claim action B, and one cron process cannot claim
   another cron. Public/internal workers have no workflow scope.
3. Core creates an invocation ID when dispatching an RPC or job. The prelude
   binds it to the per-call context and includes it on collection,
   single-statement SQL, and transaction-begin messages. The supervisor accepts
   capabilities only while that invocation is active; the ID controls lifetime
   but is never the source of the action ID.
4. Transaction begin copies the worker's immutable action scope and invocation
   lifetime into the transaction session. Completion, timeout, or worker exit
   rolls back its still-open transactions.
5. Before `SET LOCAL ROLE rootcx_app_executor`, Core sets these transaction-local
   values using bound parameters:

   - `rootcx.invocation_kind`: `action`, `job`, or empty;
   - `rootcx.invocation_name`: action ID, declared cron schedule name, or empty;
   - `rootcx.action_id`: action ID for compatibility, otherwise empty.

   The executor retains no permission to call `set_config` or change roles.
6. For jobs, the scheduler resolves `cron_schedules.name` from a Core-authored,
   top-level cron provenance field in the queue envelope. Payload fields such
   as `type` or `cron_id` never confer authority. For example, the Kova schedule
   names are `stock-minimum-purchase-proposals` and
   `notification-outbox-delivery`; their payload operation names are not used.
7. RPC/job completion or timeout closes the invocation capability and rolls
   back its open transactions. Workflow-scoped workers must announce protocol
   v4 before dispatch. Legacy workers remain compatible for internal RPCs but
   fail closed when a declared action or trusted cron requires workflow scope.

Adversarial tests cover concurrent isolated actions, replay after completion,
attacker-chosen raw IPC IDs, SQL and collection propagation, whole-transaction
pinning, Core-derived jobs versus forged payload provenance, public/internal
calls, legacy protocol behavior, and attempts to call `set_config` from app
SQL.
