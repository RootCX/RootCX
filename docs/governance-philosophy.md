# Governance Philosophy

How authority works on the platform. Not how the code is written, but what the rules ARE.

---

## The One Principle

**No principal, no authority.** Every action on the platform is performed BY a named identity (a human, an agent, or a service account). There is no "system does it", no ambient privilege, no anonymous authority. If the system cannot name who is responsible, the action is denied.

---

## Principals

There are three kinds of principals. They share the same permission engine; the difference is how they authenticate and who manages them.

| Kind | What it is | How it authenticates | Who manages it |
|------|-----------|---------------------|----------------|
| Human | An employee | Password, OIDC, magic-link | Self (registers) or admin (invites) |
| Agent | An app's AI brain | Never (the platform spawns it) | Auto-created at app deploy |
| Service account | A robot identity for automation | API key (`rcs_...`) | Admin via the SA management API |

A principal that is **disabled** loses all authority instantly, on the very next request or job execution. No waiting for token expiry.

---

## Apps and Ownership

An **app** is the unit of work: it has data (tables), logic (backend), an agent (optional), crons, hooks, webhooks, and secrets.

**Self-service rule:** any employee with the `platform:apps.create` permission can install an app. The installer automatically becomes the **app admin** (receives `app:{id}:*`). This gives full control over the app: data, crons, hooks, jobs, invoke, deploy. Platform admins (`*`) retain control over everything.

**Isolation:** owning app A gives zero authority over app B. The permission namespace `app:crm:*` cannot match `app:billing:*`. This is structural, enforced at the database level (RLS), not just at the API.

---

## Permissions

A permission is a string like `app:crm:contacts.read` or `admin:apps.deploy`. The format is `namespace:scope:action`.

**Wildcards:** `app:crm:*` grants everything under `app:crm:`. The global wildcard `*` grants everything (platform super-admin). A wildcard never leaks into another namespace.

**Deny-by-default:** a principal starts with zero permissions. Authority is explicitly granted, never assumed.

**Roles** carry permissions. A role can inherit from other roles. A principal is assigned one or more roles; their effective permissions are the union of all their roles' permissions.

---

## Who Can Do What (the full matrix)

### Data (read/write/delete records)

The employee who holds `app:{id}:{entity}.read` sees rows. The one who doesn't sees zero rows (silent filter, not an error). Same for create/update/delete.

An **app admin** (`app:{id}:*`) sees and manages all data in their app.

A **platform admin** (`*`) sees everything across all apps.

### Invoke (trigger an app or agent)

Requires `app:{id}:invoke`. This gates: calling an RPC, invoking an agent, and enqueuing a job.

### Crons (scheduled tasks)

Creating a cron requires `app:{id}:cron.write`. The cron's **owner** (the `created_by`) is the identity under which the cron executes. When it fires, the owner must still be enabled and the delegation must still be valid.

### Hooks (data-triggered automation)

Creating a hook on an app requires authentication. The hook's owner is the creator. When the hook fires, same rules as crons: owner must be enabled, delegation valid.

### Webhooks (external event triggers)

Registered at deploy time. The deployer is the owner. Inbound webhooks that trigger an agent require a valid delegation from the owner to the agent.

### Jobs (one-shot background work)

Enqueuing requires `app:{id}:invoke`. The job runs under the enqueuing user's identity (RLS).

### Integration actions (Gmail, Slack, etc.)

Requires `integration:{id}:{action}`. The action runs as the authenticated caller, using their own connected credentials. No impersonation.

### Platform operations

| Action | Requires |
|--------|----------|
| Install app | `platform:apps.create` or `admin:apps.install` |
| Deploy backend/frontend | `admin:apps.deploy` |
| Manage platform secrets | `admin:secrets.manage` |
| Manage RBAC globally | `*` (super-admin) |
| Manage service accounts | `admin:service_accounts.manage` |
| Start/stop workers | `*` |
| Manage identity providers | `*` |
| Execute raw SQL | `admin:db.query` |

---

## Agents and the Intersection Rule

An agent is an AI that acts on behalf of a human. It is never autonomous: every action it takes is bounded by BOTH what the agent is allowed to do AND what the responsible human is allowed to do.

```
Effective authority = agent's permissions INTERSECT human's permissions
```

If the agent has `[crm:*, billing:invoices.read]` and the human has `[crm:contacts.read]`, the agent can only do `[crm:contacts.read]`. The agent cannot exceed the human, and the human cannot exceed the agent.

This applies to every trigger: user-click, cron, hook, webhook, channel message, and sub-agent invoke.

---

## Delegation

A **delegation** is the standing authorization that allows a trigger (cron, hook, webhook, channel) to fire an agent on behalf of an owner.

- Created automatically when the trigger is set up.
- Revoked automatically when the trigger is deleted.
- Checked at every execution: if revoked or expired, the trigger is blocked.

A delegation does not give the agent MORE power. It merely confirms "this owner consents to this trigger activating this agent on their behalf."

---

## Service Accounts and `runAs`

A **service account** (SA) is a robot identity that owns automation. Its purpose: decouple "who set it up" from "what identity it runs under." When an employee leaves, the SA stays; the automation keeps running.

**Creating a SA:** requires `admin:service_accounts.manage`. The SA starts with zero permissions (an admin assigns a least-privilege role).

**Using a SA as automation owner (`runAs`):**

When creating a cron, hook, or job, the creator can pass `runAs: <sa_id>`. This means "own this automation as the SA, not as me." Two checks gate this:

1. **Act-as delegation:** the creator must have a standing delegation to the SA (granted by an admin).
2. **Anti-escalation:** the SA's permissions must be a subset of the creator's. You cannot point automation at a more-privileged identity than yourself.

Once the automation is created, it runs under the SA's identity. The creator can leave; the automation survives. The SA can be disabled at any time to kill all its automation instantly.

---

## The Six Triggers (how an agent starts)

| Trigger | Responsible principal | What's checked |
|---------|----------------------|----------------|
| User clicks "invoke" | The user | User has `invoke` permission |
| Cron fires | The cron owner | Owner enabled + delegation valid |
| Hook fires (data change) | The hook owner | Owner enabled + delegation valid |
| Webhook arrives | The webhook owner | Owner enabled + delegation valid |
| Channel message (Slack/Telegram) | The linked user | Linked account exists + delegation valid |
| Agent sub-invokes another agent | Same human as parent | Parent's intersection includes target's `invoke` |

In ALL cases: if the check fails, the agent does not start. Fail-closed.

---

## Three Layers of Defense

The permission system is not the only protection. Three independent layers ensure that even if one is bypassed, the others hold:

1. **Process sandbox:** the app has no database credentials, no JWT, no access to system files. It communicates only via IPC.
2. **Restricted DB role:** the app's SQL runs under a role that cannot DDL, cannot call `set_config`, cannot read system tables.
3. **Row-Level Security:** every table has forced RLS. The policies check the permission GUCs set by the core BEFORE dropping to the restricted role. The app cannot override them.

---

## Credential Lifecycle (Service Accounts)

- **Format:** `rcs_` prefix + 256-bit random secret. Shown once at creation. SHA-256 hashed at rest.
- **Expiry:** configurable (default 90 days, max 365). Expired = denied.
- **Rotation:** multiple active credentials per SA for zero-downtime rotation. Revoke the old after deploying the new.
- **Revocation:** instant. A revoked credential never works again.
- **Disable SA:** instant kill switch. All tokens and automation under this SA stop immediately.

---

## What Happens When Someone Leaves

| Scenario | What happens |
|----------|-------------|
| Employee owned crons directly | Cron's `created_by` becomes NULL (ON DELETE SET NULL) → cron immediately denied |
| Employee owned crons via a SA (`runAs`) | SA keeps running. Another admin takes over management. Nothing breaks. |
| Employee had act-as to a SA | Their delegation is inert (they can't authenticate anymore). SA continues. |
| Employee was platform admin | Their admin role is removed with their account. Other admins remain. |
| Employee was app admin | Their `app:{id}:admin` role is removed. Another admin can re-assign or the app stops being managed. |

---

## Summary of Invariants (what is ALWAYS true)

- No principal = denied.
- Disabled = denied (instantly, not at token expiry).
- Empty intersection = zero authority.
- No delegation = trigger blocked.
- No owner on a cron/hook/job = refused.
- App code cannot forge, override, or read identity GUCs.
- Wildcard `app:A:*` never matches `app:B:*` or `admin:*`.
- Anti-escalation: you cannot create automation more powerful than yourself.
- Audit: every action records who did it and who authorized it.
