# Governance Philosophy

How authority works on the platform. This document states what the rules ARE, at the level of guarantees, not how the code implements them.

---

## Vocabulary (fixed meanings)

These terms mean exactly this, everywhere in this document:

- **Principal:** a named identity that can act. One of three kinds (human, agent, service account).
- **Owner:** the principal that a piece of **owned automation** (a cron, hook, webhook, or job) runs as. Recorded as `created_by`, or a service account named via `runAs`.
- **Responsible principal:** the principal an agent run is attributed to and bounded by. For owned automation it is the owner; for a user click it is the user; for a channel message it is the linked user; for a sub-invoke it is the same as the parent.
- **App admin:** a principal holding `app:{id}:*`. This is about controlling an app, not about being an automation owner. The two are independent.
- **Sensitive action:** an irreversible or high-impact action (bulk delete, sending external mail or messages, payments, granting access, deploying).

---

## The One Principle

**No principal, no authority.** Every action on the platform is performed BY a named principal. There is no "system does it", no ambient privilege, no anonymous authority. If the platform cannot name who is responsible, the action is denied.

---

## Principals

Three kinds of principals share one permission engine. They differ only in how they authenticate and who manages them.

| Kind | What it is | How it authenticates | Who manages it |
|------|-----------|---------------------|----------------|
| Human | An employee | Password, OIDC, magic-link | Self (registers) or admin (invites) |
| Agent | An app's AI brain | Never (the platform spawns it) | Auto-created at app deploy |
| Service account | A robot identity for automation | API key (`rcs_...`) | Admin via the SA management API |

A **disabled** principal loses all authority instantly, on its very next request, job, or trigger of any kind. No path is exempt and there is no waiting for token expiry.

---

## Apps and Ownership

An **app** is the unit of work: it has data (tables), logic (backend), an agent (optional), crons, hooks, webhooks, jobs, and secrets.

**Self-service rule:** any employee holding `platform:apps.create` can install an app. The installer automatically becomes the **app admin** of that app (receives `app:{id}:*`): full control over its data, crons, hooks, jobs, invoke, and deploy. This grant is over the new app only. It never confers platform authority, and it does not let the installer approve their own elevation to `admin:*` or `*`. Platform admins (`*`) retain control over everything.

**Isolation:** holding `app:A:*` gives zero authority over app B. The namespace `app:crm:*` cannot match `app:billing:*`. This is structural, enforced at the database level, not just at the API.

---

## Permissions

A permission is a string like `app:crm:contacts.read` or `admin:apps.deploy`, in the form `namespace:scope:action`.

**Wildcards:** `app:crm:*` grants everything under `app:crm:`. The global wildcard `*` grants everything (platform super-admin). A wildcard never leaks into another namespace.

**Deny-by-default:** a principal starts with zero permissions. Authority is explicitly granted, never assumed.

**Roles** carry permissions. A role can inherit from other roles. A principal is assigned one or more roles; their effective permissions are the union of all their roles' permissions.

**Bounded granting:** you cannot grant a permission you do not hold. Assigning a role to any principal (human or service account) is bounded by the assigner's own authority. The single exception is the platform super-admin (`*`), the trusted root, which is unbounded by definition. This one rule is what makes anti-escalation hold everywhere (see Service Accounts and the invariants).

---

## The Intersection Rule (how much an agent may do)

An agent is an AI that acts on behalf of a responsible principal. It is never autonomous. Every action it takes is bounded by what the agent may do AND what the responsible principal may do AND the scope of the current task:

```
run authority = agent's permissions
              INTERSECT responsible principal's permissions
              INTERSECT task scope
```

If the agent has `[crm:*, billing:invoices.read]`, the responsible principal has `[crm:contacts.read]`, and the task scope is unset, the run authority is `[crm:contacts.read]`. Neither side can exceed the other; the task scope can only narrow further.

**Task scope** is set at invoke time (which entities, which action classes, optional time or row bounds) and expires when the run ends. Absent an explicit scope, a run is bounded to read-only plus the single action its trigger names. Authority is task-bound and ephemeral, never ambient.

**Authority only narrows down a chain.** When an agent sub-invokes another agent, the child does not inherit the parent's full set. It runs at:

```
effective(child) = effective(parent)
                 INTERSECT child agent's permissions
                 INTERSECT child task scope        (child task scope ⊆ parent task scope)
```

Each hop is a subset (⊆) of the hop above it; authority is monotone non-increasing and can only shrink. A child's task scope can never widen the parent's. There is no path by which a deep sub-agent reaches the responsible principal's full permissions if any intermediate agent was narrower. Chains have a bounded maximum depth, and a cycle (A invokes B invokes A) regains nothing: re-intersecting a set already applied changes it by nothing, so a loop runs at constant, non-growing authority until the depth bound stops it.

---

## Triggers and Ownership

Every way an agent can start is the same mechanism: a **responsible principal** activates the agent, and the agent runs at the intersection above. Triggers differ only in how that principal is identified and whether the principal may be a service account (`runAs`).

| Trigger | Responsible principal | `runAs` SA allowed? | How identified |
|---------|----------------------|:---:|----------------|
| User clicks "invoke" | The user | No (live human) | Authenticated session |
| Cron fires | The cron owner | Yes | `created_by` or `runAs` SA |
| Hook fires (data change) | The hook owner | Yes | `created_by` or `runAs` SA |
| Webhook arrives | The webhook owner | Yes | Registrar at deploy time, or `runAs` SA |
| Job runs (one-shot) | The job owner | Yes | Enqueuer, or `runAs` SA |
| Channel message (Slack/Telegram) | The linked user | No (live human) | Account link |
| Agent sub-invokes another agent | Same as parent | Inherited | Inherited from parent |

The owned-automation triggers (cron, hook, webhook, job) all carry a stored owner and obey one identical execution gate:

> the owner must be enabled AND still hold `invoke` on the app, otherwise the trigger is refused.

If the owner is absent (a deleted human leaves `created_by` empty), the automation is owner-less and refused. Fail-closed, with no exception.

The live-human triggers (user click, channel message) have no stored owner: the human is present and is the responsible principal directly. They cannot be `runAs` a service account.

---

## Delegation

There is no separate delegation object. A trigger's existence IS the standing consent: by creating a cron, hook, webhook, or job, or by linking a channel account, the responsible principal authorizes that trigger to fire its agent on their behalf. Deleting the trigger (or unlinking the account) revokes that consent.

So "consent valid" is exactly the execution gate above: the responsible principal is enabled AND still holds `invoke`. A trigger never grants the agent more power than the responsible principal has; the run stays within the intersection.

---

## Service Accounts and `runAs`

A **service account** (SA) is a robot identity that owns automation. Its purpose: decouple "who set it up" from "what identity it runs under." When an employee leaves, the SA stays and the automation keeps running.

**Creating a SA:** requires `admin:service_accounts.manage`. The SA starts with zero permissions; an admin assigns it a least-privilege role, bounded by the assigner's own authority (a non-root admin cannot grant a SA a permission they do not themselves hold).

**Using a SA as automation owner (`runAs`):** when creating a cron, hook, webhook, or job, the creator may pass `runAs: <sa_id>` to own the automation as the SA rather than as themselves. One check gates this:

- **Anti-escalation:** the SA's permissions must be a subset of the creator's. You cannot point automation at an identity more privileged than yourself.

There is deliberately no `actAs`-style "you may impersonate this SA" grant. In GCP (`iam.serviceAccounts.actAs`) and AWS (`iam:PassRole`) that grant is the number-one privilege-escalation vector, precisely because it lets a principal point at a more-privileged identity. The subset rule removes the vector by construction, with one check.

**No escalation by drift.** Because both creating a SA-owned automation (subset of the creator) and widening a SA's role (bounded by the assigner) are capped at the actor's own authority, no non-root principal can ever produce automation more powerful than themselves, at creation or later. A SA-admin cannot inflate a SA beyond their own permissions, so the seam where a creator could later widen "their own" SA is closed. Only `*` (the trusted root) is unbounded. At runtime the SA's own current permission set is the ceiling, and an agent under it is still bounded by `agent ∩ SA`.

Once created, the automation runs under the SA's identity. The creator can leave; it survives. Disabling the SA is an instant kill switch for everything it owns.

---

## Sensitive Actions and Step-Up

A sensitive action is denied by default even when it is inside the run authority. Holding the permission is necessary but not sufficient. How the action is authorized depends on whether a human is present:

- **Attended trigger (user click):** the action proceeds only on explicit live human confirmation (human-in-the-loop), or under a standing pre-authorization held by that user.
- **Unattended trigger (cron, hook, webhook, job, channel):** the action proceeds only if the responsible principal (the owner) holds a **standing pre-authorization** that names the exact action class and its bounds (count, amount, recipient). The pre-authorization belongs to the owner; a deep sub-agent may perform a sensitive action only if its class was pre-authorized by the owner, never because untrusted input asked for it.

If no valid authorization exists, the action is **refused** (fail-closed) and an approval request is raised to a configured approver: the owner if human, or the named human approver or role attached to a service-account owner. A run never blocks indefinitely waiting on a human; it fails closed, records the denial, and may be retried once approval is granted.

An agent can always do LESS than its intersection. It can never do something irreversible merely because the permission string allows it.

---

## Trigger Authenticity

Authorization answers "is this owner allowed?". Authenticity answers "is this event real?". An externally originated event must be proven authentic before it can activate any owner's authority:

- **Inbound webhooks** are cryptographically signed and timestamped. Unsigned, altered, or stale (replayed) payloads are rejected before any owner or agent is reached. The lookup secret is stored only as an irreversible hash, never in clear.
- **Channel messages** (Slack/Telegram) are verified against the platform's signing secret before the linked user is resolved. The channel is untrusted transport, not an identity.
- **Crons and jobs** originate inside the platform; their authenticity is the scheduler itself.

A payload never carries authority. It can only activate an authority the owner already holds.

---

## Untrusted Input and the Confused-Deputy Boundary

The intersection bounds the MAXIMUM an agent may do. It does not, by itself, stop an agent from being steered into a harmful action that is inside that maximum. A malicious instruction hidden in a record, an email, a webhook payload, or a prior session is the agent-era confused deputy: the attacker supplies the words, the agent acts with the owner's authority.

All app data, integration payloads, and channel content are **untrusted input**. The rule:

- Untrusted input may inform an agent's reasoning. It may never be the sole source of authority for a sensitive action.
- The authority context is the originating request (who invoked, for what task). Content the agent merely read cannot widen it.
- Any sensitive action reached on the basis of untrusted input falls under the step-up above.

Permissions alone cannot close this gap; the step-up and the task scope are the controls that do.

---

## Three Layers of Defense

Even if the permission engine were bypassed, two independent layers below it still hold:

1. **Process sandbox:** the app has no database credentials, no identity token, and no filesystem access. It communicates only through the brokered channel.
2. **Restricted execution role:** the app's SQL runs as a non-owner database role that cannot change schema, cannot set or read the identity context, and cannot reach another app's data or privileged engine internals.
3. **Row-Level Security:** every table enforces RLS for all roles, including table owners. The core sets the caller's identity for the duration of a single transaction, below the app's reach, so the identity cannot be forged, overridden, or leaked to another caller sharing a connection.

---

## Credential Lifecycle (Service Accounts)

- **Format:** typed prefix `rcs_` plus a high-entropy (256-bit) random secret. Shown once at creation; stored only as an irreversible hash.
- **Expiry:** configurable (default 90 days, max 365). Expired = denied.
- **Rotation:** multiple active credentials per SA for zero-downtime rotation. Revoke the old after deploying the new.
- **Revocation:** instant. A revoked credential never works again.
- **Disable SA:** instant kill switch. All credentials and automation under the SA stop immediately.

---

## What Happens When Someone Leaves

Deprovisioning handles the leaver. The mover (role change) is handled by recertification below: old access is never assumed to lapse on its own.

| Scenario | What happens |
|----------|-------------|
| Owned automation held directly (cron/hook/webhook/job under the human) | The owner becomes absent on account deletion, the automation is owner-less, and it is refused at the next fire. Fail-closed. |
| Owned automation held via a service account (`runAs`) | The SA owns it, not the person. Their departure changes nothing; another admin manages the SA and the automation keeps running. |
| Linked channel account | The link goes inert (the human can no longer authenticate); messages on it are refused. |
| Platform admin | Their admin role is removed with their account. Other admins remain. |
| App admin | Their `app:{id}:*` is removed. Another admin can re-assign it, or the app stops being managed. |

To survive a departure, automation must be owned via a `runAs` service account rather than directly. Directly-owned automation is meant to stop when its owner is gone.

---

## Access Recertification

Correct at grant time is not enough; standing access must be re-attested over time, or it accumulates (privilege creep).

- Every grant has an owner of record and a review cadence: privileged grants (`*`, `admin:*`, any `app:{id}:*`) are reviewed on a short cycle; ordinary access on a longer one.
- A review is an explicit decision (keep, reduce, or revoke) by someone other than the holder, and the decision is recorded.
- Access with no passing review by its deadline is flagged, and privileged access is auto-suspended pending review.
- Service-account roles, `runAs` bindings, and standing pre-authorizations are recertified the same way.

---

## Separation of Duties

No single identity both performs an action and can erase its record, nor both requests and approves its own privilege.

- The identity that administers RBAC is not the identity that administers the audit log. `admin:audit.read` is read-only; no permission can alter or delete audit records (see Audit Integrity).
- Granting a privileged role (`*`, `admin:*`) is a maker-checker action: proposed by one admin, approved by a different one. Toxic combinations (for example RBAC-admin plus audit-admin on one identity) are refused.
- Self-service install grants `app:{id}:*` over the new app only, never platform authority, and never a self-approved elevation.

---

## Audit Integrity

The audit log is the backbone of named accountability; if it can be edited, accountability is fiction.

- Records are **append-only** for every principal, including `*`. No path, raw SQL included, can update or delete them with write access.
- Records are **tamper-evident** (chained so any edit or gap is detectable) and streamed to a write-once external sink, so an instance compromise cannot rewrite history.
- Every record carries: who acted, who authorized (the responsible principal whose standing consent activated the action; equal to the actor for a direct action), the trigger, the action, the target, the task scope, and the result (allowed or denied).

---

## Who Can Do What (the matrix)

### Data (read/write/delete records)

A principal holding `app:{id}:{entity}.read` sees matching rows; one who does not sees zero rows (a silent filter, never an existence-leaking error). For create, update, and delete, an unauthorized attempt is refused explicitly (an error, not a silent no-op): a write cannot "silently see nothing." An app admin (`app:{id}:*`) manages all data in their app; a platform admin (`*`) sees everything across all apps.

### Invoke

Requires `app:{id}:invoke`. This gates calling an RPC, invoking an agent, and enqueuing a job.

### Crons, Hooks, Webhooks, Jobs (owned automation)

Creating a cron requires `app:{id}:cron.write`; a hook requires `app:{id}:hook.write` (a hook can fire on every row change, so it is gated like a cron); a webhook is registered under `admin:apps.deploy`; enqueuing a job requires `app:{id}:invoke`. Each records an owner (the creator, or a `runAs` SA) and obeys the single owned-automation gate at execution: owner enabled and still holding `invoke`, else refused.

### Integration actions (Gmail, Slack, etc.)

Requires `integration:{id}:{action}`. The action runs as the connected principal, using their own credentials, with no impersonation. The outbound credential is audience-restricted to that one integration (a Gmail token cannot be replayed against another). In automation, a SA-owned trigger may use only integration credentials the SA itself has connected; there is no borrowing of a departed human's connections. With no connection, the action is denied, never run silently as someone else.

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
| Execute raw SQL | `admin:db.query` (never write access to the audit log) |

---

## Summary of Invariants (what is ALWAYS true)

- No principal = denied.
- Disabled = denied instantly, on the next request, job, or trigger of any kind. No path is exempt, and it never waits for token expiry.
- Empty intersection = zero authority. A run never exceeds `agent ∩ responsible principal ∩ task scope`.
- Owned automation with a disabled, non-invoking, or absent owner = refused. The trigger's existence is its only consent; there is no separate delegation object.
- Authority is monotone non-increasing down a sub-agent chain; a child can never regain what an earlier hop dropped, a cycle gains nothing, and depth is bounded.
- A payload never carries authority; only an owner does. Unauthenticated external events are dropped before any owner or agent is reached (internal crons and jobs are authentic by origin).
- Untrusted input may inform an agent but is never the sole source of authority. A sensitive action is denied unless explicitly authorized (live confirmation when attended, a standing scoped pre-authorization when unattended), even inside the intersection. It never blocks forever; it fails closed.
- App code cannot forge, override, or read the identity context; identity is scoped to a single transaction and never leaks across a shared connection.
- Wildcard `app:A:*` never matches `app:B:*` or `admin:*`.
- You cannot grant, create, or later inflate authority (for a principal or a service account) beyond what you yourself hold. Only the platform super-admin (`*`), the trusted root, is unbounded.
- Integration credentials run as their connected principal, are audience-restricted to one integration, and are never borrowed across principals.
- Privileged access, `runAs` bindings, and standing pre-authorizations are recertified; unreviewed privileged access is auto-suspended.
- No identity both performs an action and can alter its record; granting a privileged role takes a second admin (maker-checker).
- Every action is recorded with who acted and who authorized it. The log is append-only and tamper-evident; not even `*` can edit or delete it.
