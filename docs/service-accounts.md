# Service Accounts

Architecture decision for non-human identities (NHI) as first-class principals.

---

## The Principle (evolved)

```
Before:  No human = no authority = denied.
After:   No principal = no authority = denied.
         Principal = human user OR managed service account. Never ambient/root.
```

Every unit of work (RPC, job, cron, hook, integration) must carry a named
principal. That principal is either a human user or a service account. There is
no anonymous, ambient, or "system" authority. Deny-by-default.

This aligns with NIST SP 800-207 (Zero Trust), Saltzer & Schroeder (complete
mediation + least privilege), Google BeyondCorp, and the OWASP Non-Human
Identities Top 10 (2025).

---

## Why

1. **Owner-orphan problem.** Today, `cron_schedules.created_by` is FK'd to a
   human with `ON DELETE SET NULL`. Human leaves = cron dies (deny-by-default
   on NULL owner). Automation tied to a person does not survive turnover.

2. **The industry consensus.** GCP: "Service accounts are not associated with
   any particular employee... think of them as resources that belong to another
   resource." GitHub Apps: "remain installed even when the person who installed
   the app leaves the organization." (Sources: Google Cloud IAM docs, GitHub
   Apps docs.)

3. **The principle must scale forever.** Binding every automation to a human is
   an over-claim the same sources contradict (Google, OWASP NHI10). A service
   account decouples "who administers" from "what identity runs."

---

## What is a Service Account

A service account is a non-human principal in `rootcx_system.users`:

| Property | Value |
|----------|-------|
| `id` | Random UUID (never derived/deterministic) |
| `email` | `sa+{slug}@localhost` |
| `is_system` | `true` |
| `kind` | `service`. New column (values `human`, `agent`, `service`); backfilled: agent rows to `agent`, other non-system to `human`. |
| `disabled_at` | Nullable timestamp (disable before delete) |
| `display_name` | Human-readable label chosen at creation |

A service account is governed by the same RBAC as humans and agents:
`rbac_assignments`, `rbac_roles`, `rbac_permissions`. No special code path.

---

## Differences from Agents and Humans

| | Human | Agent | Service Account |
|---|---|---|---|
| Created by | Signup / OIDC / magic-link | Auto at app deploy | Admin via `admin:service_accounts.manage` |
| Can login interactively | Yes | No | No |
| Tied to an app | No | Yes (1 per app) | No (standalone) |
| Survives employee departure | N/A | Depends on delegation | Yes (owned by tenant) |
| Default role at creation | `admin` (first user) or none | `admin` (first deploy, then governed) | **None** (deny-by-default) |
| Authenticates how | Password / OIDC / magic-link | Never (core spawns its worker) | Client Credentials (see below) |

---

## Ownership Model

A service account belongs to the **tenant** (the core instance), not to any
human. It is managed by any human holding `admin:service_accounts.manage`.

This decouples:
- **Who administers** the SA (humans with `manage` + `actAs` permissions).
- **What identity runs** (the SA itself, with its own least-privilege role).

When a human admin leaves, their personal `actAs` grant is revoked with their
account. The SA continues to run. Another admin takes over management. No
automation breaks.

---

## The act-as Guard (non-negotiable)

To create automation (cron, job, hook) that runs as a service account, or to use
`x-run-as` to act as another principal, a human must hold a standing **act-as
delegation** to that principal.

Act-as is NOT a permission key. `service_account:{uuid}:actAs` cannot exist: the
permission charset `[a-z0-9_:.*]` (keys CSV-encode into the `rootcx.effective_perms`
GUC) excludes the hyphens of a UUID and the uppercase of `actAs`. Act-as reuses
the existing `delegations` table: a row `(delegator_uid = human, delegatee_uid =
SA, trigger_type = 'act_as', trigger_ref = NULL)`, gated by
`delegations::is_valid(human, sa)`. Same concept as the human -> agent mandate;
the only schema change is generalizing the column `agent_uid` -> `delegatee_uid`
(a delegatee is any non-human principal: agent or service account).

This mirrors:
- GCP `iam.serviceAccounts.actAs` (Service Account User role).
- AWS `iam:PassRole` (scoped to specific role ARNs).
- Kubernetes RBAC `bind`/`escalate` verbs.

### Anti-escalation rule (Kubernetes-style, hard-enforced)

```
A human CANNOT assign a role to a SA, or create automation running as a SA,
if that SA's effective permissions exceed the human's own permissions.
```

Without this rule, a low-privilege human creates a SA with admin role and
points a cron at it = privilege escalation. This is the exact confused-deputy
hole GCP closed ("we now require that these services check that users have
permission to impersonate service accounts when attaching them to resources").

Enforcement: at every `run_as` site (cron/job/hook creation), the core checks
(a) the act-as delegation exists and (b) the target's resolved permissions are a
subset of the human's, reusing the existing `resolve_permissions` +
`intersect_permissions`. Explicit override via `admin:rbac.escalate` (high-risk,
audited). Role assignment is not a separate enforcement point: it is restricted
to super-admins (holders of `*`), for whom the subset check is always satisfied,
so adding it there would never deny anything.

### One act-as path (no duplicate)

The legacy `x-run-as` header (an admin-only impersonation on integration calls,
with no anti-escalation, used by nothing) is **removed**. Integration actions now
always run as the authenticated caller; acting for a connected user is handled
in-process by `execute_self_action` (requester-scoped); owning automation is
handled by `run_as` on crons/jobs/hooks. There is exactly one way to run as
another principal: the act-as delegation bounded by anti-escalation.

---

## Authentication (for external M2M calls)

When a service account needs to call the Core API externally (not just be the
owner of internal automation), it authenticates via OAuth 2.0 Client
Credentials (RFC 6749 section 4.4).

### Flow

```
POST /api/v1/auth/token
Content-Type: application/x-www-form-urlencoded

grant_type=client_credentials&client_id={sa_id}&client_secret={secret}
```

Response:
```json
{ "access_token": "eyJ...", "token_type": "Bearer", "expires_in": 3600 }
```

- No refresh token (RFC 6749: "MUST NOT issue a refresh token" for client
  credentials). The client re-requests on expiry.
- Access token TTL: 1 hour (configurable down to 5 min).
- Token is a standard JWT (`sub` = SA user_id), accepted by the existing
  `Identity` extractor without modification.

### Credential format

Pattern: `prefix + random`, shown once, hashed at rest. Same proven pattern as
Stripe (`sk_`/`rk_`), GitHub (`ghp_`/`ghs_`).

| Property | Value |
|----------|-------|
| Prefix | `rcs_` (rootcx service) |
| Entropy | 32 bytes (256 bits), base64url |
| Full key | `rcs_` + 43 chars (shown once at creation) |
| Storage | SHA-256 hash + prefix (for indexed lookup) |
| Prefix stored | `rcs_` + the first 8 token chars (12 chars), plaintext, for the indexed lookup and secret-scanning |
| Lookup | Index on prefix column, then constant-time hash compare. The `client_id` (a public UUID) is matched first; only the secret compare is constant-time, which is correct since the id is not secret (RFC 6749). |

Why SHA-256 (not bcrypt/argon2): the key has 256 bits of entropy; security
comes from randomness, not hash slowness. bcrypt per API request = unnecessary
latency. bcrypt/argon2 remain correct for human passwords (low entropy).

Implementation reuses the shipped `auth/secure_tokens.rs` primitive (already used
for share and magic-link tokens): `generate()` (32 bytes OsRng, base64url 43
chars), `hash()` (SHA-256), `verify()` (constant-time), `prefix()`. No new crypto;
the `rcs_` prefix is prepended to the generated token.

### Multiple active keys (zero-downtime rotation)

A SA can have multiple active credentials simultaneously. Roll the new key
into your system, then revoke the old. No window of broken auth.

### Credential table

```sql
CREATE TABLE rootcx_system.sa_credentials (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    sa_user_id  UUID NOT NULL REFERENCES rootcx_system.users(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    prefix      TEXT NOT NULL,
    key_hash    TEXT NOT NULL,
    expires_at  TIMESTAMPTZ,
    revoked_at  TIMESTAMPTZ,
    last_used_at TIMESTAMPTZ,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_sa_creds_prefix ON rootcx_system.sa_credentials (prefix);
```

---

## No Interactive Login (structurally barred)

All human login paths (password, magic-link, OIDC) create/authenticate
`is_system = false` users. A SA (`is_system = true`, `kind = service`) has no
`password_hash`, no `oidc_sub`, no magic-link. There is no code path that
produces a session/token for a SA via the interactive login routes. Auth is
only via client-credentials endpoint.

---

## Audit (double identity)

When a SA performs an action, the audit log records:
- **Actor**: the SA (`user_id`, `kind=service`).
- **Originator**: the human who created the automation (from `delegations`
  table or `created_by` on the cron/hook/job).

This mirrors GCP `serviceAccountDelegationInfo` and AWS `SourceIdentity`.
The human who set things up remains traceable even after they leave.

---

## Using a SA as automation owner

### Crons

```
POST /api/v1/apps/{app_id}/crons
Authorization: Bearer <human-token>

{
  "schedule": "*/10 * * * *",
  "payload": { "method": "sync_pipeline" },
  "run_as": "<sa_user_id>"
}
```

The human must hold `app:{id}:cron.write` AND a valid act-as delegation to the SA
(see the act-as guard).
`created_by` = SA user_id. The cron survives the human's departure. The
scheduler resolves the SA as `RpcCaller` and runs under its RLS identity.

### Jobs (enqueued by frontend or API)

Same pattern: `POST /api/v1/apps/{app_id}/jobs` with `"user_id": "<sa_id>"`.
Human must be admin or hold `actAs` on the SA + the job permission. The job
runs under the SA's identity and RLS.

### Hook/webhook owner

Same: `created_by` can be a SA, guarded by `actAs`.

---

## Lifecycle

| State | Behavior |
|-------|----------|
| Active | Normal operation |
| Disabled (`disabled_at` set) | All token issuance refused; all job/cron execution denied (deny-by-default on disabled). Re-enable by clearing `disabled_at`. |
| Deleted | Cascade: credentials deleted, cron `created_by` set NULL (denied), delegations removed. Not recoverable. |

Best practice (GCP): disable first, delete only after confirming nothing
depends on it.

---

## Permissions introduced

| Permission | Who holds it | What it gates |
|---|---|---|
| `admin:service_accounts.manage` | Admins | Create, disable, delete SAs; manage their credentials |
| act-as delegation (human -> SA) | Specific humans | Use this SA as `run_as`/`x-run-as`. Stored in `delegations`, not a permission key (see act-as guard). |
| `admin:rbac.escalate` | Super-admins only | Override the anti-escalation rule (assign a SA more perms than self) |

---

## What does NOT change

- RLS engine: unchanged (keys on `user_id`, SA is just another user_id).
- RBAC tables: unchanged (SA gets a role assignment like any principal).
- Worker identity: unchanged (`config.identity` can be a SA).
- Job scheduler: unchanged (resolves owner via `resolve_caller`, works for SA).
- Delegation table: reused (human -> SA, like human -> agent); column `agent_uid` generalized to `delegatee_uid`.
- Audit log: unchanged schema (already records `user_id` as actor).
- Credential hashing: reused (`auth/secure_tokens.rs`), not reinvented.
- `is_system` column: already exists, reused.

---

## OWASP NHI Top 10 Coverage

| # | Risk | Mitigation |
|---|------|-----------|
| NHI1 | Improper Offboarding | SA owned by tenant, disable-before-delete, not tied to human lifecycle |
| NHI2 | Secret Leakage | Shown once, SHA-256 at rest, prefix for secret-scanning |
| NHI4 | Insecure Authentication | Client Credentials (RFC 6749), JWT short-lived |
| NHI5 | Overprivileged NHI | Born with NO permissions; dedicated least-privilege role; anti-escalation rule |
| NHI7 | Long-Lived Secrets | Mandatory expiry on credentials, short-lived access tokens, rotation support |
| NHI8 | Environment Isolation | One SA per purpose (not shared cross-env) |
| NHI9 | NHI Reuse | Guidance: one SA per integration/automation, not shared |
| NHI10 | Human Use of NHI | No interactive login; act-as audited separately |

---

## References

- NIST SP 800-207 (Zero Trust Architecture): https://csrc.nist.gov/pubs/sp/800/207/final
- GCP Service Accounts: https://docs.google.com/iam/docs/service-account-overview
- GCP actAs: https://docs.google.com/iam/docs/service-accounts-actas
- AWS PassRole: https://docs.aws.amazon.com/IAM/latest/UserGuide/id_roles_use_passrole.html
- AWS AssumeRole: https://docs.aws.amazon.com/STS/latest/APIReference/API_AssumeRole.html
- Kubernetes RBAC (escalate/bind): https://kubernetes.io/docs/reference/access-authn-authz/rbac/
- Azure Service Principals: https://learn.microsoft.com/en-us/entra/identity-platform/app-objects-and-service-principals
- OWASP NHI Top 10 (2025): https://owasp.org/www-project-non-human-identities-top-10/2025/top-10-2025/
- RFC 6749 (OAuth 2.0): https://datatracker.ietf.org/doc/html/rfc6749
- GitHub token format: https://github.blog/engineering/platform-security/behind-githubs-new-authentication-token-formats/
- Stripe API keys best practices: https://docs.stripe.com/keys-best-practices
