# Governance Model

Single source of truth for who can do what, under which identity, with what authority.
Derived from the implementation on branch `governance-refactor` (2026-05-30).

---

## Core Principle

```
Effective authority = grant(agent) intersect perms(responsible human)
No human = no authority = denied.
```

Every data operation passes through PostgreSQL RLS. The core sets the identity;
the app cannot forge, override, or bypass it.

---

## The 6 Agent Triggers

| # | Trigger | Responsible human | Pre-check | Authority | On deny |
|---|---------|-------------------|-----------|-----------|---------|
| 1 | User invokes agent (HTTP) | The user who clicked | User has `app:{id}:invoke` | intersection(agent grants, user perms) | 403 |
| 2 | Cron fires | The person who created the cron | (a) owner exists, (b) delegation valid | intersection(agent grants, cron-creator perms) | Job refused, logged |
| 3 | Entity hook fires (data change) | The person who created the hook | (a) owner exists, (b) delegation valid | intersection(agent grants, hook-creator perms) | Job refused, logged |
| 4 | Channel message (Slack/Telegram) | The person who linked their account | (a) linked account exists, (b) delegation valid | intersection(agent grants, linked-user perms) | Agent silent |
| 5 | Inbound webhook (Stripe, etc.) | The person who registered the webhook | (a) owner exists, (b) delegation valid | intersection(agent grants, webhook-creator perms) | 403 |
| 6 | Agent calls another agent (sub-invoke) | Same human as the parent agent | Parent intersection includes `app:{target}:invoke` | New intersection(target agent grants, same human perms) | Error returned to parent |

---

## Data Access Paths

| # | Actor | Identity for RLS | Mechanism | On deny |
|---|-------|-----------------|-----------|---------|
| 7 | User via HTTP CRUD | The user (JWT) directly | RLS checks user permissions from DB | 0 rows (silent filter) |
| 8 | User via federated query | The user directly | Same RLS, each app schema filtered independently | 0 rows for unauthorized schemas |
| 9 | Agent reads data (query_data tool) | Human responsible + intersection | RLS checks against the intersection list (not full human perms) | 0 rows |
| 10 | Agent writes data (mutate_data tool) | Human responsible + intersection | Same RLS on writes | Postgres error |
| 11 | App calls ctx.sql("...") | The user of the current RPC/job | Core executes with identity posed, RLS filters | 0 rows or Postgres error |
| 12 | App calls ctx.collection() | The user of the current RPC/job | Same as ctx.sql but structured ops | 0 rows or error |
| 13 | Cross-app RPC (user calls another app) | The calling user | User must have `app:{target}:invoke` | 403 |
| 14 | Anonymous / share-token visitor | No identity (empty) | App must declare RPC as public | 401 or 403 |
| 15 | Standard webhook (non-agent RPC) | The webhook creator | Creator identity posed, RLS filters | Deny-all if no owner |
| 16 | Scheduled job (non-agent) | The job creator | Creator identity posed, RLS filters | Job refused if no owner |

---

## Control-Plane Operations

| # | Action | Permission required | Who can do it | On deny |
|---|--------|--------------------:|---------------|---------|
| 17 | Deploy backend | `admin:apps.deploy` | Admins | 403 |
| 18 | Deploy frontend | `admin:apps.deploy` | Admins | 403 |
| 19 | Install app | `admin:apps.install` (bypassed on first boot only) | Admins (or the very first user) | 403 |
| 20 | Uninstall app | `admin:apps.install` | Admins | 403 |
| 21 | Manage secrets (app + platform) | `admin:secrets.manage` | Admins | 403 |
| 22 | Execute admin SQL | `admin:db.query` | Admins (SELECT only, max 500 rows) | 403 |
| 23 | View schema structure | Authenticated (no specific perm) | Any logged-in user | 401 |
| 24 | Manage MCP tool servers | `admin:mcp.manage` | Admins | 403 |
| 25 | Manage agent config/sessions | `admin:agents.manage` | Admins | 403 |
| 26 | Start/stop workers | `*` (super-admin) | Super-admins | 403 |
| 27 | Create/update/delete crons | `app:{id}:cron.write` | Users with the permission | 403 |
| 28 | Create/delete entity hooks | Authenticated | Any logged-in user | 401 |
| 29 | Create/delete webhooks | `app:{id}:webhook.read` (list) | Users with the permission | 403 |
| 30 | Execute integration action | `integration:{id}:{action}` | Users with the permission | 403 |

---

## Delegation Lifecycle

| Event | Delegation created | Delegation revoked |
|-------|--------------------|--------------------|
| Admin creates a cron for an agent | Automatically (creator -> agent, type "cron") | When the cron is deleted |
| Admin creates a hook for an agent | Automatically (creator -> agent, type "hook") | When the hook is deleted |
| Admin creates a webhook for an agent | Automatically (creator -> agent, type "webhook") | When the webhook is deleted |
| User links their Slack/Telegram | Automatically (user -> agent, type "channel") | When the link is removed |

Once revoked, the automated trigger is immediately blocked. The agent cannot start.

---

## The Intersection Formula

```
What the agent can actually do =
    What the agent ROLE allows
    intersect
    What the HUMAN can do

Examples:

  Agent has [crm:*, billing:invoices.read]
  Human has [crm:contacts.read]
  -> Effective = [crm:contacts.read]

  Agent has [crm:contacts.read]
  Admin has [*]
  -> Effective = [crm:contacts.read]  (agent is bounded even by admin)

  Agent has [billing:*]
  Human has [crm:*]
  -> Effective = []  (no overlap = zero authority = deny all)
```

---

## Three Layers of Defense

```
LAYER 1: SANDBOX (process isolation)
  - App has no DB credentials (env cleared)
  - App has no JWT (auth_token removed)
  - App runs as UID 1001 (cannot read core secrets)
  - Communication only via IPC

LAYER 2: RESTRICTED ROLE (Postgres)
  - rootcx_app_executor: no DDL, no set_config, no system schemas
  - Cannot read rootcx_system, pgmq, cron schemas
  - Cannot escalate (SET ROLE blocked)

LAYER 3: RLS (row-level security)
  - Every table has FORCE ROW LEVEL SECURITY
  - Policies call check_access() which reads the GUCs
  - The core sets GUCs BEFORE dropping to restricted role
  - App cannot override (set_config revoked, SET blocked, single-statement enforced)
```

If layer 1 falls, layer 2 protects. If layer 2 falls, layer 3 protects.
All three together = no exfiltration path.

---

## Fail-Closed Invariants

| Condition | Result |
|-----------|--------|
| No user identity in the transaction | RLS denies all rows |
| Agent with empty intersection | is_delegated='1' + effective_perms='' -> deny all |
| Cron/hook/webhook without owner | Job refused before agent starts |
| Delegation revoked | Trigger blocked, agent does not start |
| Unknown context_token after onStart | Hard deny ("access denied: no user context") |
| App sends SET/DDL/DO SQL | Rejected before reaching Postgres |
| App calls set_config() | Permission denied (revoked from role) |
