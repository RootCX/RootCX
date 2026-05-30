# Governance Architecture (final)

Status: design, not yet implemented. No code in this document.
Builds on: `docs/agent-identity-rearchitecture.md` (the agent delegation model). This
document generalizes that model to every caller (human and agent) and to every hop.

All file:line references were read from the code on the current `main`.

---

## 1. The principle

> The core governs the **identity that travels**, never the app. Every access is
> evaluated against the **originating caller's** effective permissions (a human user,
> or an agent acting on behalf of a user). An app is a conduit and never lends its own
> authority. Same rule for humans and agents. Refuse by default.

This is the recognized industry pattern: OAuth2 Token Exchange delegation (RFC 8693,
the `act` claim), the textbook defense against the confused-deputy problem, and the
OWASP microservices identity-propagation guidance. It is not novel and not an
anti-pattern. The codebase already implements it for agents; the work is to make it
universal and to enforce it at the one hop where it is currently skipped.

### Two layers, not one

```
Layer A  WORKLOAD IDENTITY  (app acts as itself)
         The app owns its schema, its secrets, its no-user-context work
         (cron, startup). Trusted-subsystem for self-data.

Layer B  USER IDENTITY PROPAGATION  (delegation)
         Every cross-app and user-initiated access carries the originating
         principal. The core enforces that principal's effective permission
         at each hop. Effective authority for an agent = grant(agent) intersect
         perms(delegator).
```

Layer A is why an app may still touch its own schema directly. Layer B is why touching
another app's data, or acting for a user, must go through the core carrying the user
identity.

---

## 2. Current state (fact-checked)

The Policy Decision Point already exists and is correct.

| Brick | Where | State |
|---|---|---|
| PDP: roles, inheritance, wildcards, intersection | `extensions/rbac/policy.rs` (`resolve_permissions` :66, `resolve_effective_permissions` :115, `effective_for_pair` :132, `intersect_permissions` :145, `has_permission` :165) | Correct, ~25 unit tests. Keep unchanged. |
| Delegation tokens (RFC 8693 shape) | `auth/jwt.rs` (`mint_delegated` :65, `act` claim :71, 120s TTL :73, aud pin :88) | Correct. Keep. |
| `act`-claim identity + audit pair | `auth/identity.rs` (`Identity` :12, `actor_pair` :22) | Keep. |
| Agent path: compute effective perms once, cache, deny-by-default tool gate | `worker.rs` (`effective_for_pair` call :311-318, cache :318, clear :402/411) + `tool_executor.rs:30` (`check_permission(tool:{name})`) + `worker.rs:501` (`unwrap_or_default` = empty on miss = deny) | Correct. This is the reference model. |
| Cross-app action call carries delegated identity | `worker_manager.rs` `AppActionCallImpl::call` :310-329 (`mint_delegated`, forwards delegated caller) | Identity is carried. |
| Scheduler agent jobs: deny no-owner + standing mandate | `scheduler.rs` `dispatch_agent_job` :38-61 (`delegations::is_valid`) | Correct. Keep. |
| Self-data IPC scoped to own schema | `worker.rs` `collection_op` :678 (`table(config.app_id, entity)`, no caller-supplied app) | Layer A working as intended. |
| Audit context | `routes/crud.rs:19` (`set_audit_context_api`) | Keep; will always be populated once every path carries identity. |

What is missing or broken (the deltas):

| Gap | Where | Effect |
|---|---|---|
| **Cross-app/user RPC carries identity but the core does NOT enforce the caller's permission** | `routes/workers.rs` `rpc_proxy` :78-110 (User branch builds `RpcCaller`, no PDP check; comment "the worker layer enforces RBAC" :86 is false) -> `worker.rs:263-281` ships it over IPC -> `backend_prelude.js:282` hands `caller` to app code | Layer B holds by app convention, not by the core. The load-bearing hole. |
| `caller: Option<RpcCaller>` | `worker_manager.rs:195,228`; `ipc.rs:22` | `None` = no identity = no check. The hole is in the type. |
| Master DSN handed to every worker | `lib.rs` -> `worker_manager.rs:126` -> `worker.rs:670` (`Discover.database_url`) -> `backend_prelude.js` `ctx.databaseUrl` | Any app can run any SQL on any schema, bypassing the core entirely. |
| `collection_op` enforces no per-user RBAC | `worker.rs:678` | Acceptable for self-data (Layer A), but it is app-level authority, not per-user. |
| Scheduler regular (non-agent) job mints a full-TTL user token | `scheduler.rs` `resolve_caller` :21-26 (`encode_access`, no `act`), ownerless falls back to `SYSTEM_USER_ID` :174 | Worker holds a replayable user token; ownerless jobs run as system (high privilege). |
| ~25 routes gated by authentication only | install/uninstall (`routes/mod.rs:143,229`), deploy (`routes/deploy.rs:44,189` = RCE), secrets (`routes/secrets.rs:19,30,43,117`), raw SQL (`routes/introspection.rs:122`), agents/mcp/channels | A valid JWT is the only gate. |
| Install promotes installer to global admin on every install | `extensions/rbac/mod.rs:118-122` | Any authenticated user -> install any app -> global admin (`*`). Worst single hole. |

---

## 3. Target architecture

One judge (the PDP, unchanged), reached through structural chokepoints. Single
responsibility per component.

```
                         ┌──────────────────────────────┐
                         │  PDP  (policy.rs, unchanged)   │  the judge
                         │  resolve_effective_permissions │
                         │  effective_for_pair, intersect │
                         └───────────────▲────────────────┘
                                         │ asked by every PEP, never ad hoc
   ┌───────────────┐   ┌────────────────┴───────┐   ┌─────────────────────┐
   │ PEP-HTTP      │   │ PEP-hop                │   │ DB boundary         │
   │ default-deny  │   │ enforce caller perm at │   │ per-app PG role     │
   │ tower layer + │   │ cross-app boundary     │   │ scoped to own schema│
   │ Authorized    │   │ (rpc_proxy + AppAction │   │ (Layer A)           │
   │ extractor     │   │  Call), reuse          │   │ master DSN removed  │
   │ (control+CRUD)│   │ effective_for_pair     │   │                     │
   └───────────────┘   └────────────────────────┘   └─────────────────────┘
```

### Components and responsibilities

- **PDP** = `policy.rs`. Decides yes/no given a permission and a caller's effective
  set. Unchanged. Single source of authorization truth.
- **AuthorizedCaller** (new type, no public constructor) replaces `caller:
  Option<RpcCaller>` (`worker_manager.rs:195,228`) and `Identity` in handlers. It
  carries `{ user_id, actor, effective_permissions }`, built only by a PEP. "No
  identity = no check" becomes uncompilable. This is the human-path analogue of the
  agent path's cached `effective_permissions` (`worker.rs:318`).
- **PEP-HTTP** = one default-deny tower layer + a route -> permission table covering
  the HTTP surface (CRUD, control-plane). A route absent from the table is refused at
  runtime (fail-closed), not merely flagged in CI. Public routes (health, login,
  share, nonce upload, webhook ingress) are an explicit allowlist. Coarse,
  URL-derivable decisions only (`app:{app}:{entity}.{verb}` is a pure function of
  method + path; verb from HTTP method, app/entity from the path).
- **PEP-hop** = the keystone. At the cross-app boundary (`rpc_proxy` and
  `AppActionCall`), enforce the **originating user's** effective permission for the
  requested method/entity before forwarding. Reuse `effective_for_pair` exactly as the
  agent tool gate does. This closes the one hole where identity is propagated but not
  checked.
- **DB boundary** = each app connects with its own Postgres role scoped to its own
  schema. Cross-app data does not flow through a cross-schema GRANT; it flows through
  PEP-hop so the user's permission is checked. The master DSN is removed from
  `Discover` and `ctx`.
- **Audit** = `set_context` (reuse). Always populated because every path now carries
  identity.

### Decisions that stay near the data (cannot live in a pre-handler layer)

These need the fetched row or the request body, so they remain in handlers behind thin
PDP helpers, made non-forgettable by requiring an `AuthorizedCaller`:

- Row ownership: `crons.rs:22` `require_owner` (needs `row_owner`).
- Per-row / per-target filtering: `crud.rs` `enrich_linked` :379, `federated_query`
  :691 (which rows to return).
- Body / share-token decisions: `sharing/guard.rs:132` `authorize_public_rpc`.

---

## 4. Data flows

### Flow 1 — user reads their own app's data (governed, simple)

```
Browser (user JWT) ── HTTP GET /apps/crm/collections/contact ──> Core
   PEP-HTTP: Authorized extractor resolves effective perms (policy.rs:115)
             check app:crm:contact.read
   handler ── SQL ──> DB        (allowed -> rows; denied -> 403)
```

### Flow 2 — user via CRM reads Billing invoices (THE cross-app case)

```
User (JWT) ── HTTP POST /apps/crm/rpc {method: getTimeline} ──> Core
  │ PEP-HTTP: Authorized extractor (effective perms of the user)
  ▼
Core ── IPC Rpc(caller = AuthorizedCaller{user, perms}) ──> CRM worker
  │ CRM needs billing data -> ctx.callApp("billing","getInvoices")
  ▼
CRM worker ── back to Core (AppActionCall) ──>
  │ PEP-hop (KEYSTONE):
  │   - mint delegated token (jwt.rs:65, already exists)
  │   - ENFORCE: does the ORIGINATING user hold app:billing:invoice.read?
  │     effective = grant(crm-agent-or-app) intersect perms(user)   (effective_for_pair)
  │   - deny by default on any error
  ▼
Core ── IPC Rpc(delegated caller) ──> Billing worker ── returns invoices
        only if the user is allowed; CRM never lends its own authority
```

Today everything above exists except the bold ENFORCE step: `rpc_proxy:78-110`
forwards identity but runs no PDP check.

### Flow 3 — agent (already correct; the reference to copy)

```
invoke ─> worker.rs:311-318  effective_for_pair(agent_uid, invoker_uid) -> cache
tool   ─> tool_executor.rs:30  check_permission(perms, "tool:{name}")  deny-by-default
cross  ─> AppActionCallImpl  mint_delegated, call core (then PEP-hop as in Flow 2)
```

The human path (Flows 1-2) must mirror this shape: resolve once, carry, enforce at the
gate.

### Flow 4 — raw SQL on own schema (flexibility preserved, isolation enforced)

```
App worker ── postgres(ctx.databaseUrl) ──> DB
   ctx.databaseUrl is now a PER-APP role scoped to the app's own schema.
   SELECT in own schema      -> allowed (joins, transactions, advisory locks: all native)
   SELECT billing.invoices   -> Postgres: permission denied for schema billing
   cross-app data            -> must use Flow 2 (or RLS later, see NOT in scope)
```

### Flow 5 — scheduled job

```
Agent job  : dispatch_agent_job  deny if no owner (scheduler.rs:38), is_valid (:49). Keep.
Regular job: short-lived delegated token (not full-TTL encode_access);
             ownerless job -> deny or minimal authority (mirror the agent path),
             never SYSTEM_USER_ID full privilege.
```

---

## 5. Edge cases and failure modes

| Case | Handling |
|---|---|
| Token replay by a worker | Short TTL only. `mint_delegated` is already 120s (jwt.rs:73). Fix the regular-job path to use a short delegated token, not full-TTL `encode_access`. |
| PEP-hop DB lookup fails mid-call | Deny. `effective_for_pair` is deny-on-error (policy.rs:137-138). |
| Transient DB error strips agent perms mid-run | Cache miss -> `unwrap_or_default` -> empty -> tool denied (worker.rs:501). Acceptable (fail-closed) but surfaces as a generic denial; log distinctly. |
| Sub-agent inherits parent authority | Must re-intersect for the TARGET agent. `SubAgentDispatch` threads `invoker_user_id` (worker_manager.rs:258,273) and `worker.rs:311-318` recomputes `effective_for_pair(target_agent, invoker)`. Verify a test pins this; do not let the parent's set leak. |
| Ownerless job | Deny (do not fall back to `SYSTEM_USER_ID`). |
| `collection_op` per-user gap | Self-data only (own schema). If per-user enforcement is needed for an app's own data, route that read through the core API or add RLS. Document per app. |
| Pooled-connection `search_path` leak | `introspection.rs:139` sets `search_path` on a pooled connection without reset. Fix when touching the data layer (use `SET LOCAL` in a transaction). |
| Public / anonymous RPC (share token) | Explicit allowlist in PEP-HTTP; everything else denied. The body/share-token decision stays in `authorize_public_rpc` (guard.rs:132). |
| Target app worker down on cross-app call | PEP-hop returns a clean error; caller sees a 5xx, not a silent empty result. |
| Cross-app reads replace SQL joins -> N+1 | Provide a batched (`$in`) read on the mediated path. Reserve RLS for perf-critical joins (NOT in scope now). |
| Per-app role -> connection count per tenant DB | N apps x small pool. Keep per-worker pools small (2-5). One DB per tenant bounds app count. Add a pooler only if a tenant grows large. |
| Middleware default-deny denies a legit route | Roll out in shadow/log-only first; promote to enforce after the table is verified against real traffic. |

---

## 6. Migration order (safer at every step, no big bang)

1. **Cut the master DSN.** Give each app a per-app role scoped to its own schema; point
   `ctx.databaseUrl` at it; remove `database_url` from `Discover`. App code unchanged.
   Largest risk drop. (`worker_manager.rs:126`, `worker.rs:670`, `ipc.rs:69-77`,
   `backend_prelude.js`.)
2. **Gate `collection_op`** to its own schema (already structural) and keep it Layer A.
3. **PEP-hop:** enforce the originating caller's permission at `rpc_proxy` (workers.rs:54)
   and `AppActionCall` (worker_manager.rs:310). Reuse `effective_for_pair`. Replace
   `caller: Option<RpcCaller>` with `AuthorizedCaller`.
4. **PEP-HTTP** in shadow/log-only, then enforce. Fix the ~25 auth-only routes by giving
   them table entries (`platform:*` for control-plane). Make `install -> admin` a
   first-boot bootstrap only; gate install/deploy/secrets/db-query/mcp behind `platform:*`.
5. **Scheduler:** short-lived delegated tokens; deny ownerless regular jobs.
6. **Later (NOT in scope):** RLS for per-user enforcement inside raw SQL.

Each step compiles, ships, and reduces attack surface on its own.

---

## 7. Test strategy (locks the guarantee)

- PDP unit tests exist (`policy.rs`). Add cases for the hop intersection
  `grant(app) intersect perms(user)`.
- Authorized extractor: table-driven `(caller perms x route) -> allow/deny`.
- **Negative matrix across ALL data paths**: one low-privilege user denied on HTTP CRUD,
  `rpc_proxy`, `collection_op` IPC, and the tool path. Proves a single decision governs
  every path.
- **Cross-app delegation test**: a user without `app:billing:invoice.read` triggers a
  CRM -> billing call; PEP-hop denies. This is the keystone test for Flow 2.
- Sub-agent re-intersection test (target agent, not parent).
- Bypass test: `Discover` / `ctx` contains no DB connection string.
- DB isolation integration test: the `app_crm` role gets `permission denied` on the
  billing schema.
- Golden snapshot of the route -> permission table (anti-regression: any new or weakened
  route breaks the snapshot and forces review).
- Ownerless-job test: regular job with `created_by NULL` is denied, not run as system.

---

## 8. NOT in scope (deliberately deferred)

- RLS / per-user enforcement inside raw SQL. Needs a non-owner connecting role and
  `FORCE ROW LEVEL SECURITY` (the owner bypasses RLS). It is the way to recover
  cross-schema SQL joins while keeping per-user authority. Add only when a real
  perf-critical join demands it.
- A core-mediated raw-SQL query API (joins/aggregates/transactions through the core).
  Large; only if joins must be both expressive and centrally governed.
- Replacing the stdin/stdout IPC transport for remote workers (separate concern).

---

## 9. What already exists (reuse, do not rebuild)

| Reuse | Why it is already the right thing |
|---|---|
| `policy.rs` PDP | Correct judge, tested. The single source of truth. |
| `jwt.rs` `mint_delegated` + `act` + 120s + aud pin | RFC 8693 delegation, already implemented. |
| Agent path effective-perms-once + deny-by-default gate | The reference model; generalize to humans, do not reinvent. |
| `AppActionCallImpl` delegated cross-app call | Carries identity already; only the permission check is missing. |
| `collection_op` own-schema scoping | Layer A (trusted subsystem) working as intended. |
| `delegations::is_valid` standing mandate | Reuse for regular jobs, not just agent jobs. |
| `set_context` audit | Reuse; becomes complete once every path carries identity. |

The net of this design: roughly 80% already exists. The load-bearing addition is
**enforcing the originating caller's permission at the cross-app hop** (PEP-hop), plus
the default-deny HTTP layer, the per-app DB role, and the control-plane fixes. The
governance model itself is not new; it is the agent delegation model made universal.
