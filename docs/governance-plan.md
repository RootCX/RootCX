# Governance Plan

The implementation plan that makes `governance-philosophy.md` real, enterprise-grade, and simple to maintain.

This document is the bridge between the philosophy (what the rules ARE) and the code on this branch (what is built today). It locks the open design decisions, fixes the coherence gaps, adds the enterprise controls a SOC 2 / ISO 27001 audit requires, and maps every change to a concrete file. Every external claim is sourced at the end.

It is grounded in a full read of the current code (`core/src/extensions/rbac`, `sql_proxy.rs`, `worker_manager.rs`, `worker.rs`, `act_as.rs`, `delegations.rs`, `scheduler.rs`, `audit.rs`, `integrations/routes.rs`, `webhooks.rs`, `channels/`) and fact-checked against AWS/GCP IAM, OAuth (RFC 8693, 8707), the 2026 IETF agent-auth drafts, MCP, OWASP Agentic Top-10 2026, NIST 800-53, SOC 2, ISO 27001, and vendor guidance (Okta, Microsoft Entra, SailPoint, Vercel, GitHub, Kubernetes, Oso, WorkOS).

---

## 0. Status at a glance

**Already solid, do not touch.** Per-principal worker isolation keyed to a frozen identity (no token handed to a worker); RLS GUCs posed with `set_config(..., is_local => true)` before the role drop, with `set_config` revoked from the executor; `FORCE ROW LEVEL SECURITY` on every app table; wildcard match with a `:` boundary; permission-key charset validation at every ingestion door; the three-layer sandbox; the SA credential lifecycle (`rcs_` + 256-bit + hash-at-rest + expiry + rotation + instant revoke/disable). These match or beat AWS/GCP baselines and are correctly implemented.

**The work splits into four buckets:**
- **P0 correctness** (the model does not hold as written): sub-agent chain re-intersects with the human, not the parent; no task scope; fire-time `invoke` not checked; inbound webhooks unauthenticated.
- **P1 enterprise blockers** (an auditor issues a finding without these): MFA, recertification cadences, event-driven mover, human credential hygiene, explicit SoD set + two-person rule, break-glass alerting, audit append-only + retention.
- **P1 model coherence** (doc and code contradict each other or themselves): the delegation-object framing, the `runAs` escalation bypass, the deploy authority gap, the unauthenticated permission-enumeration endpoint.
- **P2 hardening / refinements** (ahead-of-field polish, not blocking): per-run resource budget, verifiable delegation token for the remote sub-invoke path, consent freshness, explicit depth ceiling.

---

## 1. Decisions locked (open questions, now answered)

These were the genuine "no right to be wrong" forks. Each is decided, with rationale and source.

| # | Decision | Ruling | Why (sourced) |
|---|----------|--------|---------------|
| D1 | **Channel message: attended or unattended?** | **Unattended by default.** A channel message can never itself authorize a sensitive action. A sensitive action under a channel trigger needs the linked user's standing pre-authorization. A live confirmation counts ONLY through an explicit, fresh interactive Approve/Reject bound to that exact action (a button event with a nonce), never inferred from a free-text message. | A chat message is asynchronous, replayable, and untrusted content; treating "they sent a message" as consent is OWASP ASI09 Human-Agent Trust Exploitation. The suspend-and-post-approval pattern is the 2026 standard. [OWASP Agentic 2026], [nNode approval gates] |
| D2 | **Default task scope when unset?** | **Read-only plus the single action the trigger names** (resolved from the trigger DEFINITION, never the payload). Not the full `agent ∩ principal`. | 2026 consensus is deny-by-default / minimal scope; "unset = maximum" is the ASI03 privilege-abuse failure. This also confirms the philosophy over the older `governance-model.md`. [OWASP ASI03], [least-privilege for agents] |
| D3 | **Is there a "delegation object"?** | **Yes, and it is a first-class, independently-revocable row in the `delegations` table.** The delegation grant is distinct from both the trigger (what fires) and the audit record (proof it happened). Revoking the grant disables every trigger that references it without deleting the triggers. This is the RFC 8693 `may_act` (authorization) vs `act` (record of occurrence) distinction. | Every standard (RFC 8693 `act`/`may_act`, the 2026 OBO draft, GCP domain-wide delegation, Okta OBO) requires an explicit recordable grant with who-authorized + scope, independently revocable for recertification. The `delegations` table implements exactly this. [RFC 8693], [OBO draft], [Okta OBO] |
| D4 | **`runAs` gate: subset only, or subset + standing grant + escalate-bypass?** | **Subset-by-default + one narrow act-as grant above it; no standing escalate-override.** If `SA ⊆ creator`, no extra grant needed. To attach a SA more privileged than the creator, exactly one path: a narrow, per-target, revocable, audited act-as delegation grant (`delegations` table, `trigger_type='act_as'`), fenced so the issuer must already hold the target authority. Remove `admin:rbac.escalate` as a permanent permission; break-glass only. | A blanket escalate-override re-creates the GCP `actAs` / AWS `PassRole` escalation vector. Subset-by-default covers the common case; the bounded act-as grant covers the legitimate case (a dedicated least-privilege SA that outlives its creator). [Tenable PassRole], [Rhino GCP privesc], [GCP actAs], [K8s escalate/bind] |
| D5 | **How is RBAC management delegated below super-admin?** | **Bounded granting.** Introduce `admin:rbac.manage`; a holder may grant/revoke roles, but bounded by their own authority (cannot grant a permission they do not hold). Only `*` is unbounded. Today RBAC management is all-or-nothing on `*`. | This is the philosophy's own "bounded granting" invariant, and it is what makes least-privilege admin, SoD, and the mover workflow possible without ha­nding out `*`. [AWS least-privilege], [NIST AC-6] |
| D6 | **Deploy authority** | **Three-way split:** author (uploads) ≠ deployer (`app:{id}:deploy`, routine) ≠ promoter (`app:{id}:promote`, production, maker-checker, no self-approve). `admin:apps.deploy` stays as the cross-app superset. Production promote is a sensitive action and reuses the step-up path. | Universal enterprise pattern: GitHub Environments required-reviewers (initiator cannot approve), Vercel "Full Production Deployment", Heroku admin-only promote, K8s deploy SA with no secrets/RBAC. [GitHub deployments], [Vercel], [Heroku], [K8s RBAC] |

---

## 2. P0 correctness fixes (the model must actually hold)

These are places where the code does not deliver a guarantee the philosophy states. They come before everything else.

### 2.1 Sub-agent chains must narrow against the PARENT, not the human

**Status: DONE (2026-06-03).** A sub-invoke now freezes the child's authority as `effective(parent) ∩ child_agent` through a single chokepoint, `policy::delegated_effective` (called by `WorkerManager::agent_invoke`); the parent's already-frozen set is threaded down from `ctx.permissions` via `AgentDispatcher::dispatch`. No path builds a delegated agent's effective set against the human anymore. Task scope (the third operand, §2.2) is not yet folded in. Regression guard: `delegated_effective_caps_sub_invoke_at_parent_not_human` (verified to fail when the defect is reintroduced).

**Bug.** On a sub-invoke, the code recomputes `effective_for_pair(child_agent, responsible_human)` (`worker_manager.rs:300`, reached via `worker.rs:451/523` and `invoke_agent.rs:34`). That intersects the child agent with the human's FULL permissions, so a child agent broader than its parent regains authority up to `child_agent ∩ human`. This violates the philosophy's monotone-non-increasing rule and is the single most dangerous defect: untrusted input steers a parent into sub-invoking a broader child that re-widens authority.

**Fix.** The child's run authority must be `effective(parent) ∩ child_agent ∩ child_task_scope`. Pass the parent's already-frozen effective set into the sub-invoke dispatch (`worker_manager.rs:384-404`) and intersect against THAT, not against a fresh human lookup. The PEP-hop check at `invoke_agent.rs:32` (gating `app:{target}:invoke`) stays, but it must gate against the parent's narrowed set and the result must be the narrowed intersection.

**Guarantee restored:** authority is monotone non-increasing down the chain; a cycle re-intersects an already-applied set and gains nothing.

### 2.2 Task scope is the missing third operand

**Gap.** `agent ∩ principal` is implemented and posed as the `effective_perms` GUC (`worker_manager.rs:300`, `sql_proxy.rs:54-72`, re-posed by `tools/query_data.rs:55`, `tools/mutate_data.rs:42`). The third operand, task scope, does not exist anywhere.

**Fix.** Add an optional `task_scope` to the invoke path (carried on `RpcCaller`/`ContextState`), intersect it in the same place the agent∩principal product is computed, and pose the narrowed set as the `effective_perms` GUC. Per D2, when unset the scope defaults to read-only plus the single action the trigger definition names. The scope attenuates down sub-invoke chains exactly like permissions (`child_scope ⊆ parent_scope`) and expires when the run ends.

### 2.3 Fire-time gate must check `invoke`, not just delegation-row existence

**Gap.** The owned-automation gate today checks owner-present + `principal_enabled` + `delegations::is_valid` (scheduler `scheduler.rs:38-71`, webhook `integrations/routes.rs:183-208`, channel `channels/routes.rs:333-337`). None checks that the owner STILL holds `app:{id}:invoke`. So an owner stripped of invoke still fires the trigger.

**Fix.** Add an `has_permission(owner, "app:{id}:invoke")` check to all three fire paths. This makes "consent valid = owner enabled AND still holds invoke" true, and it is what makes the mover (2.? / 3.3) effective: revoking a role instantly stops the owner's triggers.

### 2.4 Inbound webhooks must be authentic; the secret must be hashed

**Gap.** `webhook_ingress` (`integrations/routes.rs:158-253`) authenticates purely by a URL-path token that is stored in CLEAR (`webhooks.rs:12-30`). No HMAC, no timestamp, no replay protection. Anyone who learns the URL replays it forever and drives the owner's agent. The channel path is already correctly signed (Slack HMAC over `v0:{ts}:{body}` with a staleness window, `channels/slack.rs:38-67`); webhooks are the outlier.

**Fix.** (a) Store only an irreversible hash of the webhook token (mirror the SA credential pattern). (b) Verify an HMAC signature over the raw body plus a timestamp, with a short freshness window, and reject stale/replayed payloads before any owner or agent is reached. This realizes the philosophy's Trigger Authenticity section for webhooks. [OWASP webhook security], [Hookdeck]

---

## 3. P1 enterprise controls to add

The philosophy is strong on runtime authority; the enterprise gaps are the identity-hygiene perimeter. Each item below is the MINIMAL design that passes audit at one-core-per-tenant scale (not a SailPoint rollout). All are small, bounded additions.

### 3.1 MFA + step-up bound to a fresh factor (biggest gap)

The doc lists password / OIDC / magic-link but never mandates a second factor. No-MFA-on-admin is an automatic SOC 2 finding.

- **Mandate MFA** (TOTP or WebAuthn) for any human holding `*`, `admin:*`, or `app:{id}:*`. Non-privileged humans: MFA strongly recommended, or inherit AAL2 from the OIDC IdP.
- **Step-up** for sensitive actions (D1, §5.3) binds the attended confirmation to a FRESH factor assertion, not a bare UI click.
- Session reauthentication: overall ≤ 24h, inactivity ≤ 1h (NIST 800-63B AAL2).
- **Code:** `extensions/auth.rs`, `routes/auth.rs` (login + a step-up/reauth endpoint); enforce at the `Identity` extractor for privileged routes.

Standard: SOC 2 CC6.1, ISO A.5.17, NIST 800-63B AAL2. [ISMS CC6.1], [NIST 800-63B]

### 3.2 Recertification cadences, written down

The doc has the right shape (owner of record, short vs long cycle, decision by a non-holder, auto-suspend of unreviewed privileged). Pin the numbers:

- **Privileged** (`*`, `admin:*`, any `app:{id}:*`, SA roles, `runAs` bindings, standing pre-authorizations): **quarterly (90 days)**.
- **Standard** (ordinary per-entity grants on humans): **annually**.
- Reviewer is a different admin or the grant's owner of record, never the holder. Record keep/reduce/revoke + reviewer + timestamp as audit events. Service accounts are explicitly in scope.
- Unreviewed-by-deadline privileged grant auto-suspends (the compensating control that lets us skip full JIT).
- **Code:** a `rbac_grants` review-metadata column set (owner_of_record, last_reviewed_at, review_due_at) + a daily sweep job; reuse the existing scheduler.

Standard: SOC 2 CC6.3, ISO A.5.18 / A.8.2, NIST AC-6(7). [ISMS A.5.18], [NIST AC-6(7)], [SOC2 cadence]

### 3.3 Event-driven mover (role change drops stale access now)

The core IS the system of record (no HRIS), so "mover" = an admin changing role assignments. Make it real:

- Role assignment edits the role set as a WHOLE (replace, not accumulate). Removing a role drops its permissions on the owner's next request (the same "no waiting" semantics as disable; already true for disable via `identity.rs:70`).
- Emit `rbac.role.granted` / `rbac.role.revoked` audit events on every change.
- **Code:** `rbac/routes.rs` assign/replace + audit hook. Combined with 2.3, a stale owner's triggers stop firing immediately.

Standard: SOC 2 CC6.3, ISO A.5.18, NIST AC-2. [SailPoint lifecycle], [ISMS A.5.18]

### 3.4 Human credential hygiene (small but mandatory)

- Password policy (length, lockout) for password logins; session timeout ≤ 24h / idle ≤ 1h.
- A CC6.2 registration-approval record: a human is registered and authorized before credentials issue (the first-boot self-promote is the one allowed exception, §6).
- **Code:** `extensions/auth.rs`, `routes/auth.rs`.

Standard: SOC 2 CC6.1 / CC6.2, NIST 800-63B. [SOC2 password], [NIST 800-63B]

### 3.5 Separation of duties: explicit toxic set + two-person rule

The doc commits to this; make it concrete.

- **Toxic combinations refused on one identity:** (1) RBAC-admin + audit-admin; (2) maker = checker on a privileged grant; (3) raw SQL (`admin:db.query`) + any audit write (already structurally impossible since audit is append-only, keep as the third leg).
- **Two-person rule** for granting/elevating to `*` or `admin:*`: proposed by one admin, approved by a different one (a two-step state machine on the grant record, not a workflow engine). Requires ≥ 2 admins per tenant; the break-glass account (§3.6) is the documented single-admin escape hatch.
- **Code:** `rbac/routes.rs` grant path + a small `pending_grants` table.

Standard: SOC 2 CC6.3, NIST AC-5 / AC-3(2), ISO A.5.3. [NIST AC-5], [NIST AC-3(2)]

### 3.6 Break-glass alerting

The `*` super-admin is the break-glass root. Make it a real control:

- ≥ 2 dedicated emergency `*` accounts, not daily-driver identities, credentials vaulted.
- **Alert on every use:** any authentication or action by a break-glass `*` account fires an immediate alert to other admins.
- Functionality + alert test every 90 days; post-use justification recorded.
- **Code:** flag the accounts; alert hook in the `Identity` path + audit.

Standard: ISO A.8.2 break-glass, Microsoft Entra emergency-access reference design. [Entra break-glass]

### 3.7 Audit: append-only, tamper-evident, retained, and covering governance changes

Today the audit log is a plain table; the per-row trigger covers APP-table data changes only, not `rootcx_system` governance mutations (`audit.rs:87-155`). It is not append-only and has no chain or external sink.

- Make `audit_log` **append-only for every principal including `*`**: revoke UPDATE/DELETE, ensure `admin:db.query` has no write path to it.
- **Tamper-evident:** chain each record (hash of the previous) so any edit or gap is detectable; stream to a write-once external sink.
- **Cover governance changes:** install the audit trigger on the governance tables (roles, assignments, service_accounts, credentials, delegations, webhooks, crons, hooks) so RBAC/SA/delegation changes are recorded, not just app data.
- **Retention** ≥ 1 year; alert rules on denials, break-glass use, and SoD-violation attempts (CC7.2).
- **Code:** `extensions/audit.rs` (GRANT lockdown, chain column, sink, trigger install on system tables, read already gated by `admin:audit.read`).

Standard: NIST AU-9 (incl. AU-9(2) separate storage, AU-9(3) crypto protection), SOC 2 CC7.2, ISO A.8.15. [NIST AU-9]

### 3.8 JIT/PIM and dedicated break-glass accounts: explicitly OPTIONAL

Full just-in-time privileged elevation is overkill at one-core scale; "least-privilege standing + auto-suspend-on-unreviewed + quarterly recert" is the accepted substitute. The cheap nod to ISO A.8.2's "expiration dates" is to add a **90-day expiry on privileged grants**, renewed at recert. Do not build a PIM elevation flow unless a customer's auditor demands zero standing privilege. [JIT is recommended-not-required]

---

## 4. Permission catalog, deployment, and bounded granting

### 4.1 Fix the permission-enumeration leak (the "RBAC enumeration HIGH" finding)

`list_available_permissions` (`rbac/routes.rs:265-273`) is UNAUTHENTICATED and returns every app's permission keys to any caller: a full reconnaissance map of the platform.

- Add an auth guard and **scope the result to what the caller can already grant** (their own namespaces). This is the Check-vs-Enumerate split (OpenFGA/Zanzibar) and is exactly bounded-granting (D5) applied to discovery: you can only see scopes you could grant.
- `Check`-style gates (`require_perm`) stay fully available; they leak nothing.

Standard / pattern: OpenFGA relationship queries (Check vs ListObjects). [OpenFGA]

### 4.2 Tiny role taxonomy with least-privilege templates

- `admin` = `[*]` (seeded, un-editable). Keep.
- `app:{id}:admin` = `[app:{id}:*]` (auto-minted for the installer). Keep.
- **Add two optional built-in templates per app at install** so least-privilege is the default path, not a custom build: `app:{id}:viewer` = all `*.read`; `app:{id}:editor` = inherits viewer + `*.create/update` + `invoke`. Mirrors the OAuth coarse+fine mix; uses the existing role-inheritance mechanism.
- **Agent default role:** change the first-deploy default from `admin` (`agents/mod.rs:172-183`) to a least-privilege role derived from the agent's declared needs in `agent.json`, never global admin. Deny-by-default applies to agents too.

Standard: AWS least-privilege (avoid `*`), OAuth scope design. [AWS least-privilege], [Auth0 scopes]

### 4.3 Deployment authority (D6)

| Authority | Permission | Notes |
|-----------|-----------|-------|
| Create/install an app | `platform:apps.create` (self-service) or `admin:apps.install` | unchanged; installer auto-gets `app:{id}:*` |
| Deploy (non-prod) | **`app:{id}:deploy`** (new per-app) | lets an app admin ship their own app without platform power; `admin:apps.deploy` remains the cross-app superset |
| Promote to production | **`app:{id}:promote` + maker-checker** | sensitive action; proposed by the deployer, approved by a different `app:{id}:promote` holder; self-approval refused |
| Manage secrets | `admin:secrets.manage` | deliberately NOT bundled into deploy (K8s "no secrets" rule) |
| Manage RBAC | `*` or bounded `admin:rbac.manage` (D5) | deploy never implies RBAC (SoD) |

A deploy/promote pipeline can `runAs` a deployment service account (survives the engineer leaving), gated by the same subset check. Production promote reuses the §5.3 step-up path verbatim, so it needs zero new mechanism. **Code:** `routes/deploy.rs:52,199`.

Standard: GitHub Environments (no self-approve), Vercel, Heroku, K8s RBAC, DevOps SoD. [GitHub deployments], [Vercel], [Heroku], [K8s RBAC]

### 4.4 Reconcile `runAs` to subset-by-default + bounded act-as (D4)

- `act_as.rs:17-41`: remove the `admin:rbac.escalate` bypass permission. Keep the subset check (`SA ⊆ creator`) as the zero-grant default path.
- The `act_as` delegation type in `delegations` table stays (it is the narrow grant for the exceed-subset case, per D4). It is bounded: the issuer must already hold the target authority. It is recertifiable as a standing grant.
- Add the missing **bounded-granting check** on `assign_role` (`rbac/routes.rs:203-218`): a non-`*` `admin:rbac.manage` holder cannot grant a permission they do not hold. This is what closes the SA-widening drift seam structurally.

---

## 5. AI agent governance (already best-in-class; finish it)

The philosophy already matches the strongest 2026 draft (`draft-niyikiza-oauth-attenuating-agent-tokens`) almost claim-for-claim. Items 5.1-5.2 are correctness (§2 covers the chain bug and task scope); 5.3 is a true gap; 5.4-5.7 are hardening.

### 5.3 Sensitive-action step-up engine (true gap)

Today only a per-agent `supervision` config exists (`worker.rs:459-516`) with `RequiresApproval`/`RateLimited`, and it can BLOCK awaiting a human rather than fail closed. Replace with the philosophy's model:

- A sensitive action is deny-by-default even inside the run authority.
- **Attended (user click):** live human confirmation bound to a fresh factor (§3.1) and a single-use nonce tied to the exact action + target + run id, max-age ~5 minutes.
- **Unattended (cron/hook/webhook/job/channel):** requires the responsible principal's **standing pre-authorization** naming the exact action class and bounds (count, amount, recipient), with `notBefore`/`notAfter` and a lifetime cap bound to the recert cadence. A deep sub-agent may perform a sensitive action only if its class was pre-authorized by the owner.
- No valid authorization: **refuse (fail closed)**, record the denial, raise an approval request to a configured approver (the owner if human; a named human approver/role for an SA owner). Never block indefinitely.
- The action set the trigger names is resolved from the trigger DEFINITION, never the payload (confused-deputy boundary).
- **Code:** new `pre_authorizations` table + a step-up checkpoint in the tool-execution path (`worker.rs:435-527`); replace the blocking branch with fail-closed.

Standard: OWASP Agentic step-up, Anthropic HITL-before-critical, Delegation Receipt Protocol freshness. [OWASP Agentic 2026], [DRP]

### 5.4 Per-run resource budget (refinement, OWASP ASI02)

Attach `budget { max_tool_calls, max_fanout, max_wall_clock_ms, max_cost }` to a run as a SINGLE SHARED pool for the whole run-tree (not per-hop, which multiplies under fan-out). On sub-invoke, each field is `min(child_requested, parent_remaining)` (monotone, like authority). Exhaustion = fail closed, partial work preserved. Wall-clock uses the server clock, never an agent timestamp. [OWASP ASI02 Tool Misuse / budget exhaustion], [attenuating-tokens]

### 5.5 Verifiable delegation token for the REMOTE sub-invoke (true blocker, remote path only)

The co-located IPC sub-invoke is computed in-process by the trusted core and is not forgeable: leave it as-is. The enterprise remote transport (WebSocket/gRPC, per the architecture notes) must cryptographically bind the child's inherited scope, or a compromised intermediary forges authority over the wire. A2A does NOT solve this (transport auth only, no delegation model). Use an attenuating token per remote hop, signed by the core:

- `del_depth` (= parent + 1), `del_max_depth` (intermediates may only lower it), `par_hash` (binds to this exact parent, prevents splicing), `jti` (chain-unique; duplicate = deny = cryptographic cycle detection), scope/tools `⊆ parent`, `exp ≤ parent.exp`.
- The receiver verifies the whole chain OFFLINE. The parent token is never forwarded; a fresh narrower token is minted (the MCP anti-passthrough rule at the token layer).
- **Ship this before enabling remote sub-invoke.** [attenuating-tokens], [WorkOS multi-hop], [MCP authorization]

### 5.6 Consent freshness (refinement)

Live confirmations: 5-minute max-age, single-use nonce bound to action+target+run, checked against server time. Standing pre-authorizations: explicit `notBefore`/`notAfter` + lifetime cap bound to recert; each use logged and rate-limited against its bounds. Verification failures cannot be overridden by any runtime/agent-supplied flag. [DRP], [OAuth transaction tokens]

### 5.7 Root-owned depth ceiling (refinement)

The root of a run-tree sets `max_depth` at invoke (the human for attended; the trigger owner for owned automation; `*` sets the platform ceiling). Every sub-invoke does `child.max_depth = min(child_requested, parent.max_depth - 1)`; intermediates may lower, never raise. Terminal when `del_depth == del_max_depth`. This makes even a re-intersecting cycle die at the ceiling. [attenuating-tokens]

---

## 6. The complete use-case matrix

Every path, the responsible principal, the gate at create time, the gate at execute time, sensitivity handling, and behavior on owner change. "Gate at execute" is always additionally: responsible principal enabled AND holds `app:{id}:invoke` (the §2.3 fix), and the run is bounded by `agent ∩ principal ∩ task scope`.

| Use case | Responsible principal | Create-time gate | Execute-time gate | On owner disabled / deleted |
|----------|----------------------|------------------|-------------------|------------------------------|
| Human clicks invoke | The user (live) | `app:{id}:invoke` | session valid + enabled + invoke | n/a (live) |
| Human enqueues job | The enqueuer | `app:{id}:invoke` | owner enabled + invoke; else RLS denies all | disabled/deleted: refused |
| Cron, human-owned | The cron owner | `app:{id}:cron.write` | owner enabled + invoke | deleted: `created_by` null = refused; disabled: refused |
| Cron, `runAs` SA | The SA | `cron.write` + `SA ⊆ creator` | SA enabled + invoke | SA disabled = all its automation dies; creator leaving = no effect |
| Hook, human-owned | The hook owner | `app:{id}:hook.write` | owner enabled + invoke | as cron |
| Hook, `runAs` SA | The SA | `hook.write` + `SA ⊆ creator` | SA enabled + invoke | as cron-SA |
| Webhook, deployer-owned | The deployer | `admin:apps.deploy` + HMAC/timestamp verified at ingress | owner enabled + invoke + authentic payload | deleted: refused |
| Webhook, `runAs` SA | The SA | deploy + `SA ⊆ creator` | SA enabled + invoke + authentic | as cron-SA |
| Channel message | The linked user | account link issues consent row | verified signature + linked user enabled + invoke; **unattended for sensitive** (D1) | unlink/disable: messages refused |
| Sub-invoke, co-located | Same as parent | parent holds `app:{target}:invoke` in its narrowed set | `effective(parent) ∩ child ∩ child_scope` (§2.1) | inherits parent |
| Sub-invoke, remote | Same as parent | attenuating token minted (§5.5) | offline chain verify + narrowed scope | inherits parent |
| Integration action, interactive | The caller | `integration:{id}:{action}` | caller's own connected creds, audience-restricted | n/a |
| Integration action, in automation | The SA owner | `integration:{id}:{action}` | SA's OWN connected creds only; no borrowing; no connection = denied | n/a |
| Sensitive action, attended | The user | action class allowed in intersection | live fresh-factor confirmation + nonce | n/a |
| Sensitive action, unattended, pre-auth present | The owner | standing pre-auth names class + bounds | within bounds + freshness | n/a |
| Sensitive action, unattended, no pre-auth | The owner | n/a | **refused, fail closed; approval request raised** | n/a |
| Install app | The installer | `platform:apps.create` or `admin:apps.install` | n/a; installer gets `app:{id}:*` | n/a |
| Deploy (non-prod) | The deployer | `app:{id}:deploy` | n/a | n/a |
| Promote to production | The promoter | `app:{id}:promote` + maker-checker (no self-approve) | sensitive-action step-up | n/a |
| Grant a role | The granter | `admin:rbac.manage` bounded by own perms, or `*` | bounded: cannot grant beyond self | revoked role drops on next request (mover) |
| Grant `*` / `admin:*` | The granter | two-person maker-checker | n/a | n/a |
| Create SA / assign SA role | An SA admin | `admin:service_accounts.manage`; role bounded by assigner | n/a | n/a |
| Rotate SA credential | An SA admin | `admin:service_accounts.manage` | multiple active creds; revoke old after deploy | n/a |
| Disable SA | An SA admin | `admin:service_accounts.manage` | instant kill of all its automation | n/a |
| Human leaver, owns automation directly | n/a | n/a | automation owner-less = refused | the point: directly-owned automation stops |
| Human leaver, owns via SA | The SA | n/a | SA continues under admin management | nothing breaks |
| Human mover (role change) | n/a | replace-not-accumulate + audit events | stale role drops on next request | immediate |
| Break-glass `*` use | The emergency account | vaulted credential | alert fires on every use; post-use justification | n/a |
| Recertification cycle | The reviewer (not the holder) | quarterly privileged / annual standard | unreviewed privileged auto-suspends | n/a |

---

## 7. Roadmap (ordered by risk, mapped to files)

**P0 (correctness, the model does not hold without these):**
1. ✅ DONE (2026-06-03) — Sub-agent narrows against parent, not human, via `policy::delegated_effective`. `worker_manager.rs`, `tools/mod.rs`, `invoke_agent.rs`.
2. Task scope as the third operand + read-only default. `RpcCaller`/`ContextState`, `worker_manager.rs:300`, `sql_proxy.rs:54-72`, `tools/*`.
3. Fire-time `invoke` check on all owned-automation paths. `scheduler.rs:38-71`, `integrations/routes.rs:183-208`, `channels/routes.rs:333-337`.
4. Webhook HMAC + timestamp + hashed token. `integrations/routes.rs:158-253`, `webhooks.rs:12-30`.
5. `runAs`: drop `admin:rbac.escalate` bypass, keep subset-by-default + bounded act-as grant for exceed-subset. `act_as.rs:17-41`.
6. Delegation matrix kind enforcement: `delegations::create()` must validate `(delegator.kind, delegatee.kind)` against permitted pairs (Part 3). Human->Human, Agent->Human, SA->Human, Agent->SA = structurally refused. `delegations.rs`, `act_as.rs`.
7. Worker invalidation on permission change: when a role is assigned/revoked, proactively kill all workers whose principal includes the affected user (they respawn lazily with fresh perms). `worker_manager.rs` (new `invalidate_for_user`), `rbac/routes.rs` (call after assign/revoke).

**P1 (enterprise blockers + coherence):**
8. MFA for privileged humans + step-up to a fresh factor. `extensions/auth.rs`, `routes/auth.rs`.
9. Audit append-only + chain + external sink + governance-table coverage + retention. `extensions/audit.rs`.
10. Bounded granting (`admin:rbac.manage`) + mover replace-semantics + `rbac.role.*` events. `rbac/routes.rs:203-218`.
11. Recertification cadences + auto-suspend sweep. `rbac` metadata + scheduler.
12. Sensitive-action step-up engine (pre-auth table, fail-closed). `worker.rs:435-527`.
13. SoD toxic set + two-person rule on `*`/`admin:*`. `rbac/routes.rs` + `pending_grants`.
14. Permission-enumeration auth guard + scope to grantable. `rbac/routes.rs:265-273`.
15. Deploy/promote split. `routes/deploy.rs:52,199`.
16. Human credential hygiene + break-glass alerting. `extensions/auth.rs`, `Identity` path.
17. Agent default role least-privilege (not admin). `agents/mod.rs:172-183`.
18. SA owner-of-record as mandatory enforced field + ownerless refused + offboarding reassigns. `users` table + `service_accounts/mod.rs` + `agents/mod.rs`.

**P2 (hardening / refinements, ship with or before remote transport):**
19. Per-run resource budget. invoke path + sub-invoke dispatch.
20. Attenuating delegation token for remote sub-invoke (BLOCKER before remote is enabled).
21. Consent freshness (nonces, max-age, lifetime caps).
22. Root-owned depth ceiling (configurable, lowered per hop, replaces hard depth-2 cap).

**Doc reconciliation:** DONE (2026-06-04). `governance-philosophy.md` is the single canonical spec, updated with D1-D6. `governance-model.md`, `service-accounts.md`, `agent-identity-rearchitecture.md`, and `security-context-token-confusion.md` are deleted (superseded/archived).

---

## 8. Why this stays simple

The plan adds capabilities, not orthogonal concepts. The teachable surface stays small because almost everything reuses primitives that already exist:

- Task scope, the resource budget, and sub-agent narrowing are all **the same intersection**, applied to more operands and down the chain. One mental model.
- The delegation grant is a **first-class row in the `delegations` table**, independently revocable. But creating a trigger auto-creates one, so the common path is zero-overhead.
- Production promote, sensitive integration actions, and destructive agent actions all go through **one step-up path**. One control.
- Bounded granting is **the subset rule** (already used for `runAs`) applied to role grants and to permission discovery. One rule, three uses.
- Mover is **role-assignment as replace** plus the disable semantics that already exist. No IGA engine.
- The enterprise perimeter (MFA, recert cadences, SoD list, break-glass alert, audit lockdown) is bounded configuration and a daily sweep job, not a platform.

---

## 9. Sources

Permission / escalation model
- AWS least-privilege IAM techniques: https://aws.amazon.com/blogs/security/techniques-for-writing-least-privilege-iam-policies/
- Tenable, auditing iam:PassRole (privesc): https://www.tenable.com/blog/auditing-iampassrole-a-problematic-privilege-escalation-permission
- Rhino Security Labs, GCP privilege escalation: https://rhinosecuritylabs.com/gcp/privilege-escalation-google-cloud-platform-part-1/
- OpenFGA relationship queries (Check vs ListObjects): https://openfga.dev/docs/interacting/relationship-queries
- Auth0 OAuth least privilege / scopes: https://auth0.com/blog/oauth2-access-tokens-and-principle-of-least-privilege/

Delegation / on-behalf-of
- RFC 8693 OAuth Token Exchange (act / may_act): https://datatracker.ietf.org/doc/html/rfc8693
- RFC 8707 Resource Indicators (audience restriction): https://datatracker.ietf.org/doc/html/rfc8707
- IETF agent on-behalf-of draft: https://datatracker.ietf.org/doc/draft-oauth-ai-agents-on-behalf-of-user/
- Okta OBO / delegation chain: https://www.okta.com/blog/ai/agent-security-delegation-chain/
- GCP domain-wide delegation best practices: https://support.google.com/a/answer/14437356
- GitHub Apps (install survives departure): https://docs.github.com/en/developers/apps/getting-started-with-apps/differences-between-github-apps-and-oauth-apps

Deployment authority
- GitHub deployments & environments (no self-approve): https://docs.github.com/en/actions/reference/workflows-and-actions/deployments-and-environments
- Vercel extended permissions (Full Production Deployment): https://vercel.com/docs/rbac/access-roles/extended-permissions
- Heroku Pipelines (admin-only promote): https://devcenter.heroku.com/articles/pipelines
- Kubernetes RBAC good practices: https://kubernetes.io/docs/concepts/security/rbac-good-practices/
- Separation of duties in DevOps / SOX: https://ismiletechnologies.com/en_us/devops/segregation-of-duties-sod-in-devops/

Enterprise governance standards
- SOC 2 CC6.1 logical access (MFA): https://www.isms.online/soc-2/controls/logical-and-physical-access-controls-cc6-1-explained/
- SOC 2 CC6 controls & quarterly cadence: https://soc2auditors.org/insights/soc-2-security-controls/
- SOC 2 password requirements: https://sprinto.com/blog/soc-2-password-requirements/
- ISO 27001:2022 A.5.18 access rights review: https://www.isms.online/iso-27001/annex-a-2022/5-18-access-rights-2022/
- ISO 27001:2022 A.8.2 privileged access (break-glass, expiry): https://www.isms.online/iso-27001/annex-a-2022/8-2-use-of-privileged-access-rights-2022/
- NIST 800-53 AC-5 Separation of Duties: https://csf.tools/reference/nist-sp-800-53/r5/ac/ac-5/
- NIST 800-53 AC-3(2) Dual Authorization: https://csf.tools/reference/nist-sp-800-53/r4/ac/ac-3/ac-3-2/
- NIST 800-53 AC-6(7) review of privileges: https://csf.tools/reference/nist-sp-800-53/r5/ac/ac-6/ac-6-7/
- NIST 800-53 AU-9 protection of audit info: https://csf.tools/reference/nist-sp-800-53/r5/au/au-9/
- NIST 800-63B Authentication Assurance Levels: https://pages.nist.gov/800-63-4/sp800-63b/aal/
- SailPoint event-driven lifecycle (mover): https://documentation.sailpoint.com/identityiq_83/help/provisioning/lifecycle_event_driven_p.htm
- Microsoft Entra emergency (break-glass) access: https://learn.microsoft.com/en-us/entra/identity/role-based-access-control/security-emergency-access

AI agent governance (2026 state of the art)
- IETF attenuating agent tokens (del_depth, par_hash, jti, monotone narrowing): https://datatracker.ietf.org/doc/html/draft-niyikiza-oauth-attenuating-agent-tokens-00
- IETF agent delegation receipts (freshness, time window, replay): https://www.ietf.org/archive/id/draft-nelson-agent-delegation-receipts-09.html
- IETF OAuth transaction tokens (5-min live, single-use): https://datatracker.ietf.org/doc/html/draft-ietf-oauth-transaction-tokens-08
- MCP authorization (no token passthrough, confused deputy): https://modelcontextprotocol.io/specification/draft/basic/authorization
- WorkOS multi-hop delegation (offline chain verify): https://workos.com/blog/oauth-multi-hop-delegation-ai-agents
- OWASP Top 10 for Agentic Applications 2026: https://genai.owasp.org/resource/owasp-top-10-for-agentic-applications-for-2026/
- OWASP Agentic benchmark (ASI03/ASI08/ASI09): https://genai.owasp.org/2025/12/09/owasp-top-10-for-agentic-applications-the-benchmark-for-agentic-security-in-the-age-of-autonomous-ai/
- Oso, authorizing AI agents: https://www.osohq.com/learn/best-practices-of-authorizing-ai-agents
- Slack/Telegram human-in-the-loop approval gates: https://www.nnode.ai/blog/2026-02-05-human-in-the-loop-approval-gates

Webhook authenticity
- OWASP / webhook security (HMAC + timestamp + replay): https://www.hooklistener.com/learn/webhook-security-fundamentals
- Hookdeck webhook security vulnerabilities: https://hookdeck.com/webhooks/guides/webhook-security-vulnerabilities-guide
