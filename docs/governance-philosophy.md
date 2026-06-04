# Governance Engine

The single source of truth for RootCX's security and governance model. It specifies what the rules ARE at the level of structural guarantees. Operational policy (review cadences, MFA mandates, break-glass procedures) lives in `governance-plan.md`.

Where the current code diverges from this document, that is a conformance gap, not a change to the rules.

---

## PART 1 — THE MODEL

### Primitives (5 concepts, nothing else)

The entire model is built from exactly these:

| # | Primitive | What it is |
|---|-----------|------------|
| 1 | **Principal** | A named identity that can act. Exactly one of three kinds: human, agent, service account. |
| 2 | **Grant** | A principal's resolved permission set (roles unioned). Deny-by-default; starts at zero. |
| 3 | **Intersection** | The universal authority operator: `effective = grant(actor) ∩ perms(responsible principal) ∩ task scope`. Monotone non-increasing. |
| 4 | **Delegation grant** | A first-class, independently-revocable object that authorizes one principal to act on another's behalf. RFC 8693 `may_act`. Distinct from the trigger (what fires) and the audit record (proof it happened). |
| 5 | **Owner of record** | The human accountable for every non-human identity. Mandatory, never orphaned. |

### Vocabulary (fixed meanings)

- **Responsible principal:** the principal a unit of work is attributed to and bounded by. It is the only thing a trigger varies.
- **Owner:** the responsible principal recorded on a stored trigger (cron, hook, webhook, job). A human (`created_by`) or a service account (`runAs`).
- **App admin:** a principal holding `app:{id}:*`. Controls an app; independent from being an automation owner.
- **Run authority:** the exact permission set a single run executes under. Always an intersection.
- **Sensitive action:** an irreversible or high-impact action (bulk delete, external mail/messages, payments, granting access, production deploy).

### The One Principle

**No principal, no authority.** Every action is performed BY a named principal. There is no "system does it", no ambient privilege, no anonymous authority. If the platform cannot name who is responsible, the action is denied.

### Principals

Two identity classes: **human** and **non-human**. Identity (who it is) is governed separately from authority (what it may do). All three kinds share one permission engine.

| Kind | Class | What it is | Authenticates | Managed by |
|------|-------|-----------|---------------|------------|
| Human | human | An individual person; the root of accountability | Password, OIDC, passkey (interactive) | Self or admin |
| Service account | non-human | A workload identity for deterministic automation that must outlive any individual | Client credentials (`rcs_...` key to short-lived token), M2M | Admin; always has a human owner of record |
| AI agent | non-human | An identity whose behaviour is **non-deterministic**: it reasons, picks tools dynamically | Platform spawns under a posed identity; or own credentials in autonomous mode | Auto-created at app deploy; always has a human owner of record |

**What distinguishes an agent from a service account is behaviour, not authority:**

| | Service Account | AI Agent |
|---|---|---|
| Behaviour | **Deterministic** (runs fixed code) | **Non-deterministic** (reasons, picks tools) |
| Authority operator | Own grant (REPLACE) | Intersection with responsible principal |
| May be a responsible principal? | Yes | Yes (in autonomous mode, governed identically to a SA) |
| Extra gate | None | **Sensitive-action step-up** (because non-determinism = confused-deputy surface) |

**An agent is dual-mode.** Acting on a principal's behalf, its authority is the intersection. Acting autonomously (cron, hook, scheduled), it runs on its own grant ∩ task scope, governed exactly like a service account. Every chain, attended or not, terminates in an accountable human via the owner of record.

A **disabled** principal loses all authority instantly, on its very next request, job, or trigger. No path exempt; no waiting for token expiry.

### Apps and ownership

An **app** is the unit of work: data (tables), backend logic, an optional agent, crons, hooks, webhooks, jobs, secrets.

**Self-service:** any employee holding `platform:apps.create` can install an app and automatically becomes its **app admin** (`app:{id}:*`). The grant is over the new app only. It never confers platform authority and never lets the installer self-approve elevation to `admin:*` or `*`.

**Isolation is structural.** `app:A:*` cannot match `app:B:*` or `admin:*`. Each app's data lives in its own schema, gated by RLS.

### Permissions

A permission is a string `namespace:scope:action` (e.g. `app:crm:contacts.read`, `admin:apps.deploy`, `integration:gmail:send`).

- **Wildcards:** `app:crm:*` grants everything under `app:crm:`; the boundary is `:`, so it never leaks into `app:crm_secret:...`. The global `*` is platform super-admin.
- **Deny-by-default:** a principal starts with zero permissions.
- **Roles** carry permissions and may inherit other roles. Effective permissions = union of assigned roles.
- **Bounded granting:** you cannot grant a permission you do not yourself hold. Only `*` is unbounded.

### The Composition Law (the heart)

Every run executes under an **intersection**:

```
run authority = agent's own grant
              ∩ responsible principal's permissions  (omitted when autonomous)
              ∩ task scope
```

- When an agent acts on a principal's behalf, the responsible principal's permissions are an operand.
- When it acts autonomously (own credentials, like a SA), that operand drops out; the run is bounded by own grant ∩ task scope.
- **Empty intersection = zero authority.**
- **Task scope** is ephemeral: at invoke time the run is bound to specific entities and action classes. Default when unset = **read-only plus the single action the trigger names** (resolved from the trigger's definition, never from a payload). Expires with the run.
- **Down a chain, authority is monotone non-increasing.** A sub-invoke intersects against the parent's already-frozen set, never re-derived from the root. A cycle re-intersects a set already applied and gains nothing; depth is bounded by a ceiling set at the root and lowered by each hop.

---

## PART 2 — THE SCENARIO CATALOG

Every unit of work: **identify the responsible principal, compute the authority, pose it to the data layer.**

### Family A — Direct access (no agent)

- **A1 — A principal reads/writes its own app data** (human or SA, same path). RLS filters: read-deny = 0 rows (silent); write-deny = error.
- **A2 — App backend code touches data** (`ctx.sql` / `ctx.collection`). Runs under the current unit of work's identity, never a fresh or wider one. App code inherits a principal; it never selects one.
- **A3 — No identity in context.** No principal = deny-all. The app may run stateless logic but reaches no governed data.

### Family B — An agent run (the intersection)

The run executes under `agent ∩ responsible ∩ task scope`.

| Trigger | Responsible principal | `runAs` SA | Fire-time gate |
|---------|----------------------|:---:|----------------|
| Human clicks invoke | The user (live) | No | user holds `app:{id}:invoke` |
| Service account invokes (M2M) | The SA | n/a | SA holds `app:{id}:invoke` |
| Cron fires | The cron owner | Yes | owner present + enabled + holds `invoke` |
| Hook fires (data change) | The hook owner | Yes | same |
| Webhook arrives | The webhook owner | Yes | same + authentic payload (Family H) |
| Job runs (one-shot) | The job owner | Yes | same |
| Channel message (Slack/Telegram) | The linked user | No | linked + enabled + holds `invoke` + authentic (Family H) |

- **B1 — The owned-automation gate.** Cron, hook, webhook, job, and channel all obey one gate: *owner present AND enabled AND holds `app:{id}:invoke`*, else **refused, fail-closed**. An owner stripped of invoke, disabled, or deleted stops firing immediately.
- **B1b — Consent is a delegation grant (RFC 8693 `may_act`).** When a trigger is created, a delegation grant captures the consent. The trigger *references* the grant. Revoking the grant immediately disables every trigger that references it without deleting them. The audit log records each exercise (`act`) but is never the authorization (`may_act`).
- **B2 — A user click carries no stored owner.** The human is present directly as the responsible principal; it cannot be `runAs` a service account.
- **B3 — Channel messages are unattended for sensitive actions.** The linked user is the responsible principal, but a channel message can never *itself* authorize a sensitive action (Family F). A message is input, not authorization.
- **B4 — Task scope applies.** Default = read-only + the trigger's named action. A payload never widens authority.

### Family C — Composition (chains and cross-app)

- **C1 — Sub-invoke.** An agent invokes another. The hop is gated against the **parent's frozen set** (`app:{target}:invoke` must be inside it). The child runs at `child_agent ∩ effective(parent) ∩ child_scope`. Authority narrows against the parent, never re-widens against the root.
- **C2 — Deep chains and cycles.** Each hop is a subset. A cycle re-intersects and gains nothing. A depth ceiling, set at the root and lowered per hop, terminates the tree.
- **C3 — Cross-app data.** RLS filters each app's schema per-principal; unauthorized schemas return 0 rows.
- **C4 — Cross-app action call.** The frozen intersection is carried to the target.

### Family D — The agent's tools

| Tool | Effect | Bound by |
|------|--------|----------|
| Read / write data | Governed query | RLS under the run authority |
| Call an integration | Outbound action | The responsible principal's own connected credential, audience-restricted. No connection = denied. |
| Call another app's action | Cross-app hop | C4 |
| Invoke another agent | Sub-invoke | C1 |

### Family E — Granting and ownership

Every way new authority is created is capped at the actor's own authority.

- **E1 — Grant a role.** Bounded granting: cannot assign a permission you do not hold. Only `*` is unbounded.
- **E2 — Create automation `runAs` a service account.**
  - **Default: subset-by-construction.** If `perms(SA) ⊆ perms(creator)`, no extra grant needed. Most automation lives here.
  - **Above subset: one act-as grant.** A narrow, explicit, SA-specific, revocable, audited delegation grant, fenced by: **no one may issue act-as on an identity more privileged than the granting authority's own.** This keeps the capability GCP/AWS deliberately keep (`actAs`/`PassRole`) while closing the unscoped escalation vector (a well-documented cloud privilege-escalation path). [Tenable, Rhino Security Labs]
  - **No standing escalate-override permission.** No `admin:rbac.escalate` as a permanent grant. If the residual case arises (creating an SA more privileged than any current admin), it is a **break-glass procedure**: time-boxed, dual-authorized, alerting, auto-expiring, reviewed at closure. Not a role anyone carries. [ISO 27001 A.8.2, Microsoft Entra emergency-access]
- **E3 — Install an app.** The installer auto-receives `app:{id}:*` over the new app only (self-service), never platform authority.

### Family F — Sensitive actions and the confused-deputy boundary

The intersection bounds the maximum. It does not stop an agent from being *steered* by malicious input into a harmful action within that maximum. So a sensitive action is **deny-by-default even inside the run authority**.

- **F1 — Attended (human present, e.g. a user click).** Proceeds only on explicit, fresh, single-use confirmation bound to the exact action and target. Never inferred from free-text.
- **F2 — Unattended (cron, hook, webhook, job, channel).** Proceeds only under the owner's **standing pre-authorization** naming the exact action class and bounds (count, amount, recipient). No valid pre-authorization = **refused, fail-closed**, recorded, approval request raised. Never blocks forever.

All app data, integration payloads, and channel content are **untrusted input**: may inform reasoning, never the sole source of authority.

### Family G — Lifecycle

- **G1 — Disable a principal.** Instant deny on next request/job/trigger. No path exempt.
- **G2 — Delete a human owner.** Directly-owned automation becomes ownerless and is refused. Directly-owned automation is meant to stop when its owner is gone.
- **G3 — Disable/delete a service account.** Kill switch for everything it owns.
- **G4 — Offboard owner of record.** A non-human identity (agent or SA) is never left ownerless. Ownership must be re-assigned; an ownerless identity is refused. [OWASP NHI1]
- **G5 — Revoke/expire a credential.** Instant and permanent. Multiple active credentials for zero-downtime rotation.
- **G6 — Revoke a role / reduce permissions.** Drops on next request, including for any automation that principal owns.

### Family H — Authenticity

External events must be proven authentic before any owner's authority is touched.

- **H1 — Inbound webhooks:** signature over raw body + timestamp, short freshness window; replayed/altered payloads dropped. Secret stored as irreversible hash.
- **H2 — Channel messages:** verified against the platform's signing secret before the linked user is resolved.
- **H3 — Crons and jobs:** authentic by origin (the scheduler itself).

A payload never carries authority. It can only activate an authority the owner already holds.

---

## PART 3 — THE DELEGATION MATRIX

Every ordered pair among {human, agent, service account}. Authority always narrows or stays equal; no transition widens.

| Initiator → Runs-as | Verdict | Authority | Gate |
|---|---|---|---|
| **Human → Human** | Forbidden | -- | No human runs as another (non-repudiation) |
| **Human → Agent** | Permitted | `agent ∩ human ∩ scope` | `app:{id}:invoke` |
| **Human → SA** | Guarded | `SA ∩ scope` (REPLACE) | Act-as grant + subset fence (E2) |
| **Agent → Human** | Forbidden | -- | Authority never re-mounts |
| **Agent → Agent** | Guarded | `child ∩ effective(parent) ∩ child_scope` | `invoke` in frozen parent; depth bound; no-cycle |
| **Agent → SA** | Forbidden (default) | -- | Non-deterministic actor cannot escape intersection clamp via REPLACE |
| **SA → Human** | Forbidden | -- | Machine never runs as human |
| **SA → Agent** | Permitted | `agent ∩ SA ∩ scope` | SA holds `invoke`. This is the "autonomous agent". |
| **SA → SA** | Guarded | `SA_target ∩ scope` (REPLACE) | Human-created act-as grant, subset-fenced, depth 1 max, recertified |

**Structural invariants on the matrix:**
- Only **human** and **service account** can be a responsible principal at the root of a chain.
- An **agent** may be its own responsible principal in autonomous mode (governed like a SA, with owner of record).
- A delegation grant's target is always non-human. No edge ever targets a human.
- Per-principal worker isolation makes each cell's identity fixed at spawn.

---

## PART 4 — STRUCTURAL ENFORCEMENT

The permission engine is not the only line. Even if bypassed, independent layers hold:

1. **Process sandbox.** The app has no database credentials, no identity token, no filesystem access. It communicates only through the brokered channel.
2. **Restricted execution role.** The app's SQL runs as a non-owner role: no DDL, no `set_config`, no system schemas.
3. **Row-Level Security.** Every table has FORCE RLS. The core poses identity for a single transaction, below the app's reach.

**Per-principal worker isolation.** Each distinct identity runs in its own worker process, bound for life to the identity it was spawned for. One identity's run can never act as another's.

---

## PART 5 — CREDENTIAL LIFECYCLE (SERVICE ACCOUNTS)

- **Format:** typed prefix `rcs_` + 256-bit random secret. Shown once; stored as irreversible hash (SHA-256), looked up by indexed prefix then constant-time compare.
- **Expiry:** mandatory, configurable (default 90 days). Expired = denied.
- **Rotation:** multiple active credentials per SA for zero-downtime. Roll new in, revoke old.
- **Revocation:** instant and permanent (G5).
- **Disable:** instant kill switch for the SA and everything it owns (G3).
- **Authentication:** client credentials exchanged for a short-lived bearer token (no refresh token, RFC 6749 section 4.4). The token carries the SA as subject.
- **No interactive login:** all human login paths (password/OIDC/magic-link) create human identities only. A SA has no password, no OIDC sub, no magic-link.

---

## PART 6 — AUDIT

The audit log is the backbone of named accountability; if it can be edited, accountability is fiction.

- Every record carries: **who acted** (the actor), **who authorized** (the responsible principal whose consent activated the action), the trigger, the action, the target, the task scope, the result (allowed/denied), and the delegation grant id consumed.
- Records are **append-only** for every principal including `*`: no path, raw SQL included, can update or delete them.
- Records are **tamper-evident** (hash-chained; any edit or gap is detectable). No identity both performs an action and can erase its record.
- Records are **retained** >= 1 year and streamed to a write-once external sink.

---

## PART 7 — INVARIANTS (what is always true)

1. **No principal = denied.** Every unit of work names a responsible principal or it does not run.
2. **Disabled = denied instantly.** No path exempt; never waits for token expiry.
3. **A run never exceeds `agent ∩ responsible principal ∩ task scope`.** Empty intersection = zero authority.
4. **Authority is monotone non-increasing down a chain.** A child never regains what a parent dropped; a cycle gains nothing; depth is bounded.
5. **Owned automation with a disabled, non-invoking, or absent owner is refused.** Consent is a first-class grant: revoking it disables referencing triggers without deleting them.
6. **You cannot grant authority beyond what you hold.** `runAs` is subset-by-default; exceeding it requires a narrow, fenced act-as grant. No standing escalate-override.
7. **Every non-human identity has a human owner of record.** An ownerless identity is refused; offboarding reassigns. Every chain terminates in an accountable human.
8. **An agent is dual-mode.** Its authority is always bounded: intersection when acting for another, own grant when autonomous. Its behavioural non-determinism is why sensitive actions require step-up.
9. **A sensitive action is denied unless explicitly authorized** (fresh confirmation when attended, standing pre-authorization when unattended). Fails closed, never blocks forever.
10. **Untrusted input informs but never authorizes.** A payload never carries authority.
11. **External events are authenticated before any principal is reached.**
12. **Integration credentials run as their connected principal**, audience-restricted, never borrowed.
13. **App code cannot forge, override, or read the identity context.**
14. **`app:A:*` never matches `app:B:*` or `admin:*`.** App isolation is structural.
15. **Every action is recorded with who acted and who authorized.** Append-only, tamper-evident, hash-chained.

---

## PART 8 — COMPLIANCE MAPPING

This model satisfies the following controls (verified against primary sources):

| Framework | Key controls satisfied |
|---|---|
| **SOC 2** | CC6.1 (identity verification, immediate revocation, audit trail), CC6.3 (periodic review, SoD, named privileged access), CC7.2 (monitoring) |
| **ISO 27001:2022** | A.5.3 (SoD), A.5.15 (access control), A.5.16 (identity lifecycle incl. non-human), A.5.17 (authentication), A.5.18 (provisioning + review + revocation), A.8.2 (privileged access, break-glass, expiry) |
| **NIST 800-53 r5** | AC-2 (account management), AC-3(2) (dual authorization), AC-5 (SoD), AC-6 (least privilege), AC-6(7) (review), AU-9 (audit protection), AU-10 (non-repudiation), IA-9 (service identification), PS-4/PS-5 (termination/transfer) |
| **NIST 800-207** | Zero-trust tenets: per-request, dynamic policy, least privilege, no implicit trust |
| **EU AI Act Art 14** | Human oversight capability, intervenability (G1/G3 disable/stop), proportionate to risk (Family F) |
| **OWASP NHI 2025** | NHI1 (offboarding/G4), NHI5 (overprivileged/deny-by-default), NHI7 (long-lived secrets/Part 5 expiry), NHI10 (human use of NHI/identity separation) |
| **OWASP Agentic 2026** | ASI01 (goal hijack/Family F untrusted-input rule), ASI02 (tool budget/per-run resource budget), ASI03 (privilege abuse/Composition Law), ASI09 (trust exploitation/Family F attended gate) |
| **RFC 8693** | `may_act` (delegation grant) vs `act` (audit record) distinction; nested actor chains |
| **IETF attenuating-agent-tokens** | `del_depth`, `par_hash`, `jti` cycle detection, monotone narrowing (Part 3, Family C) |

---

## SOURCES

Permission / escalation model:
- AWS least-privilege IAM: https://aws.amazon.com/blogs/security/techniques-for-writing-least-privilege-iam-policies/
- Tenable, auditing iam:PassRole: https://www.tenable.com/blog/auditing-iampassrole-a-problematic-privilege-escalation-permission
- Rhino Security Labs, GCP privilege escalation: https://rhinosecuritylabs.com/gcp/privilege-escalation-google-cloud-platform-part-1/
- OpenFGA relationship queries: https://openfga.dev/docs/interacting/relationship-queries

Delegation / on-behalf-of:
- RFC 8693 OAuth Token Exchange: https://datatracker.ietf.org/doc/html/rfc8693
- RFC 8707 Resource Indicators: https://datatracker.ietf.org/doc/html/rfc8707
- RFC 6749 OAuth 2.0 (client credentials): https://datatracker.ietf.org/doc/html/rfc6749
- IETF agent OBO draft: https://datatracker.ietf.org/doc/draft-oauth-ai-agents-on-behalf-of-user/
- IETF attenuating agent tokens: https://datatracker.ietf.org/doc/html/draft-niyikiza-oauth-attenuating-agent-tokens-00
- GCP actAs: https://cloud.google.com/iam/docs/service-accounts-actas
- Kubernetes RBAC (escalate/bind): https://kubernetes.io/docs/reference/access-authn-authz/rbac/

Enterprise governance:
- SOC 2 CC6.1: https://www.isms.online/soc-2/controls/logical-and-physical-access-controls-cc6-1-explained/
- ISO 27001:2022 A.5.18: https://www.isms.online/iso-27001/annex-a-2022/5-18-access-rights-2022/
- ISO 27001:2022 A.8.2: https://www.isms.online/iso-27001/annex-a-2022/8-2-use-of-privileged-access-rights-2022/
- NIST 800-53 AC-5/AC-6: https://csf.tools/reference/nist-sp-800-53/r5/ac/ac-5/
- NIST 800-63B AAL: https://pages.nist.gov/800-63-4/sp800-63b/aal/
- NIST 800-207 Zero Trust: https://csrc.nist.gov/pubs/sp/800/207/final
- Microsoft Entra emergency access: https://learn.microsoft.com/en-us/entra/identity/role-based-access-control/security-emergency-access

AI agent governance:
- EU AI Act Article 14: https://artificialintelligenceact.eu/article/14/
- OWASP Top 10 for Agentic Applications 2026: https://genai.owasp.org/resource/owasp-top-10-for-agentic-applications-for-2026/
- OWASP Non-Human Identities Top 10: https://owasp.org/www-project-non-human-identities-top-10/2025/top-10-2025/
- Okta agent identity governance: https://www.okta.com/blog/ai/ai-agent-identity-governance/
- WorkOS multi-hop delegation: https://workos.com/blog/oauth-multi-hop-delegation-ai-agents
- MCP authorization: https://modelcontextprotocol.io/specification/draft/basic/authorization

Webhook authenticity:
- OWASP webhook security: https://www.hooklistener.com/learn/webhook-security-fundamentals
