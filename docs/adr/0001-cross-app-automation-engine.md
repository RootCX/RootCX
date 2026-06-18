# ADR 0001: Cross-app automation engine as a Core-native second executor

Status: Accepted (2026-06-18)
Deciders: Sandro

## Context

We want a visual, n8n-style automation/workflow engine whose primary use case is
cross-app orchestration over the internal tools and AI agents deployed on RootCX.
Open question: build it in the Core daemon, in the `rootcx-website` control plane,
or as a deployed app.

Verified ground truth (independent 5-agent fact-check, 2026-06-18):

- The Core `ToolRegistry` (`core/src/tools/`) already is a governed capability
  library. Each `Tool` exposes a `ToolDescriptor { name, description, inputSchema }`
  and runs through a `tool:{name}` permission gate (`tool_executor.rs:31`) plus
  per-user RLS via `begin_app_tx` (`governance/enforcement/sql_proxy.rs:85`). The
  agent loop is an LLM-driven executor over this registry.
- Identity is posed by the Core: `begin_app_tx` sets RLS GUCs and drops to the
  non-login role `rootcx_app_executor` (NOBYPASSRLS, `set_config` revoked).
  `effective_perms` is a frozen 3-way intersection (principal, responsible human,
  task_scope), monotone non-increasing (`worker_manager.rs:331`).
- Anti-escalation is enforced by one gate `assert_can_act_as` (target perms must be
  a subset of the human's, `act_as.rs:31`). Triggered automations pass
  `fire_gate::assert_can_fire` (owner present, enabled, valid delegation, still holds
  `app:{id}:invoke`, fail-closed). No responsible human means denied
  (`scheduler.rs:148`).
- `rootcx-website` is the multi-tenant control plane (provisions 1 Core + 1 Postgres
  per tenant on EKS, bridges identity via OIDC token-exchange to each Core). It holds
  no per-tenant business data and uses a separate auth model. It is not a runtime.
- Triggers funnel through one pgmq `jobs` queue and a single scheduler. Entity-change
  triggers already exist (`entity_hooks` plus a SECURITY DEFINER Postgres trigger that
  enqueues to pgmq). BUT: the queue has no retry and no dead-letter, regular jobs are
  never archived on success (re-delivery risk), and there is no delayed / one-shot
  execution primitive.

## Decision

1. The engine is a Core-native extension `core/src/extensions/workflows/`: a second,
   deterministic DAG executor over the existing `ToolRegistry`. Not the website, not a
   deployed app.
2. Binding rationale: only the Core can pose per-user identity and enforce the
   frozen-intersection governance on every node. A deployed app would have to run as a
   broad-permission principal (the RBAC-enumeration anti-pattern flagged RED in the
   governance release review) or re-implement the enforcement layer over HTTP.
3. "Automation" is an app capability (a kind), mirroring "Agent = app with a brain".
   Automation = app with a wiring diagram. It gets a deterministic principal and a
   least-privilege role, like agents. Cross-app reach equals its owner's permissions,
   RLS-enforced per node.
4. Governance is inherited, with zero new authority surface. Each node runs through the
   deepened tool-dispatch under a `ToolAuthority { invoker_user_id = responsible human,
   effective_perms = intersect(automation_role, human, scope) }`. Manual run is run-as
   caller; triggered run is run-as owner via `fire_gate`.
5. Authoring: workflows are live-edited (the DB is the source of truth, with JSON
   export/import), unlike code-deployed agents. The automation principal and role are
   minted at workflow create-time.
6. Durability is built, not inherited: `workflow_executions` + `node_runs` (Postgres)
   are the source of truth; pgmq is a lease/wake mechanism only. Completion is modeled
   on the agent path (archive-on-Done), never the regular-job path. Retry/backoff,
   dead-letter, and wait/delay (`wake_at`) are new.
7. Prerequisite deepening (ADR scope): extract a deep, executor-agnostic tool-dispatch
   module (`tools::dispatch` returning a `ToolOutcome`) with agent-IPC, HTTP, and
   workflow as thin adapters. This also closes the existing HTTP fail-open gap (the
   direct execute path skips the `tool:{name}` check).

## Consequences

- Positive: governance is inherited per node (RLS, fire-gate, intersection, audit).
- Positive: on-record-change triggers are nearly free (extend `entity_hooks` with
  `action_type='workflow'` plus a scheduler branch).
- Positive: the dispatch deepening fixes a pre-existing HTTP permission asymmetry.
- Cost: durability, retry, dead-letter, and wait must be built; the queue lacks them.
  This is the bulk of the engine work.
- Touch points: `worker.rs` and `tools/routes.rs` (dispatch seam), `scheduler.rs`
  (payload-kind dispatch). A new scheduler branch is mandatory for any new job kind:
  an uncabled `action_type` is silently inert today.

## Build order

1. Tables, shared types, tool-to-node adapter, permission-filtered palette, and the
   dispatch deepening. Manual run only, run-as caller.
2. Executor: item dataflow, restricted expression evaluator, control nodes
   (If/Switch/Merge/Set).
3. Durable run module: PG state machine, pgmq lease, per-node retry, completion on the
   agent-path model.
4. Triggers: schedule (`cron_schedules`), record-change (`entity_hooks`), webhook;
   run-as owner via `fire_gate`.
5. Wait/Delay (`wake_at`), Loop, Sub-workflow, Code node (sandbox), channel and
   integration triggers.
6. Inspector UI, versioning, partial re-run.
