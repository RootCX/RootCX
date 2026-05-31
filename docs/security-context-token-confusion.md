# Security: Cross-User Context Token Confusion

**Status:** FIXED (2026-05-31) via per-principal worker isolation (Option A). Token
abolished; workers keyed by `(app_id, identity)`, identity fixed at spawn. Verified
by `worker_cannot_act_as_another_user` (api_integration) + `identity_key` unit tests.
See "Resolution & Remaining Follow-ups" at the end. Original report below.

**Status (original):** Open (blocker for "non-bypassable per-user governance" claim)
**Discovered:** 2026-05-31, governance-refactor branch security review
**Severity:** HIGH (privilege escalation, per-user RLS bypass)
**Affected invariants:** #2 (per-user RLS), #4 (identity set by core, not app)

---

## Summary

A malicious app can read or write data under the identity of another user who
happens to use the same app concurrently. The root cause is that one worker
process serves all users of an app, and nothing binds a data-access request to
the unit of work it belongs to.

---

## Architecture Context

```
User A ──┐                        ┌── PostgreSQL (RLS)
User B ──┤── HTTP ──► Core ──IPC──► Worker (1 per app)
User C ──┘                        └── (untrusted code)
```

- One worker process per app (`worker_manager.rs:138`, keyed by `app_id` only).
- The worker serves ALL users of that app concurrently (enterprise model:
  one core, many studios, many users).
- Apps are explicitly untrusted (governance-model.md, Layer 1).

---

## How Identity Flows Today

1. User makes a request (RPC, Job, AgentInvoke).
2. Core generates a **context token** (256-bit random, `sql_proxy.rs:50-55`)
   and stores a mapping `token -> (user_id, permissions, delegated?)` in a
   per-worker in-memory map (`worker.rs:191`, `context_states`).
3. Core sends the token **in plaintext** to the worker inside the IPC message
   (`worker.rs:290, 312, 347`).
4. When the app calls `ctx.sql()` or `ctx.collection()`, the prelude sends
   the token back to the core.
5. Core looks up the token in `context_states` (`worker.rs:589-592`), poses
   the corresponding identity as PostgreSQL GUCs, drops to the restricted
   role, and executes the SQL under RLS.

---

## The Vulnerability

Step 5 performs a **simple map lookup**. There is no check that the token
presented belongs to the unit of work currently being processed. The core
cannot distinguish "this SqlQuery is part of Alice's RPC" from "this SqlQuery
is part of Bob's RPC" -- both tokens coexist in the same map, and the worker
chooses which one to echo.

### Preconditions for exploitation

1. The app is malicious or compromised (threat model: apps are untrusted).
2. Two or more users with different privilege levels invoke the app within
   the token TTL window (60 seconds, refreshed on use).
3. The app sends a `SqlQuery` or `CollectionOp` IPC message using a token
   that belongs to a higher-privilege concurrent user.

### Exploitation scenario

```
t=0ms   Alice (low-priv) calls the app
        -> Core mints token_A (identity=Alice, perms=[contacts.read])
        -> Core sends token_A to worker in Rpc message

t=200ms Admin calls the same app
        -> Core mints token_B (identity=Admin, perms=[*])
        -> Core sends token_B to worker in Rpc message

t=300ms Worker (handling Alice's request) sends:
        { type: "sql_query", context_token: "token_B", sql: "SELECT * FROM hr.salaries" }

        Core looks up token_B -> finds Admin identity -> poses Admin GUCs
        -> RLS opens all rows -> returns salary data to the worker
        -> Worker returns it as part of Alice's response
```

Result: Alice sees data she has no permission to access.

### Why the core cannot verify

The worker multiplexes N requests in a single event loop. From the core's
perspective, it receives sequential IPC messages with no reliable
"this message belongs to request X" correlation. Any such correlation would
have to come from the worker itself, which is untrusted -- a malicious worker
would simply lie.

---

## What governance-model.md Claims vs. Reality

| Claim (governance-model.md) | Reality |
|---|---|
| "App calls ctx.sql() -- The user of the current RPC/job" (row 11) | The user **whose token is presented**. If the app is honest, same thing. If not, any concurrent user. |
| "The core sets the identity; the app cannot forge, override, or bypass it" (line 16-17) | The app cannot **forge** a token (256-bit random), but it can **select** among tokens it legitimately holds for concurrent users. |
| "Every data operation passes through PostgreSQL RLS" (line 15) | True -- but RLS enforces the identity the core posed, and the core posed an identity the app chose from its pool. |

---

## Existing Mitigations (insufficient)

| Mitigation | Why it does not close the gap |
|---|---|
| Tokens are 256-bit random (unguessable) | The worker does not need to guess -- it receives tokens in plaintext for every user it serves. |
| Tokens expire after 60s of inactivity | The app can keep a token alive by issuing SQL on it, or simply exploit it within the 60s window. |
| Tokens are removed on completion | The app can delay returning the response (up to the 30s RPC timeout) to keep the window open. |
| Agent invokes use intersection (delegated) | Only protects the agent path. Direct RPCs pose the full user identity (`is_delegated: false`). |

---

## Fix Options

### Option A: Per-principal worker isolation (recommended)

Key `workers` by `(app_id, user_id)` instead of `app_id` alone. Each worker
process holds exactly one user's token at a time. Cross-user confusion becomes
structurally impossible.

**Trade-offs:**
- N users x M apps = N*M processes (memory, startup cost, onStart per user).
- The "one agent per app" model forks into "one agent instance per (app, user)".
  Agent session history becomes per-user (which may actually be desirable).
- Fleet/status/stop APIs need to handle the multiplied worker set.
- Idle-timeout and pooling strategy needed to bound resource usage.

**Effort:** Large (multi-day). Touches worker_manager, worker supervisor loop,
agent invoke, fleet APIs.

### Option B: Document as a known limitation

State in governance-model.md that per-user isolation holds only for
**trusted** apps or **single-user** deployments. For untrusted multi-user
apps, the app can act as any concurrent user.

**Trade-offs:**
- Zero engineering effort.
- Contradicts the core value proposition ("governed infrastructure for
  untrusted apps"). Cannot claim "non-bypassable" in marketing or sales.
- Acceptable only if all deployed apps are first-party / audited.

### Option C: Make RPC path delegated+intersected (defense-in-depth)

Today only agent invokes compute `effective_perms = grant(agent) intersect
perms(human)`. Extend this to direct RPCs: the worker's maximum authority for
any user is the intersection of the app's declared scope and the user's perms.

**Trade-offs:**
- Reduces the blast radius (attacker gets intersection, not full user perms).
- Does NOT close the gap: the app can still read User B's intersection-scoped
  data while serving User A. Per-user isolation is still broken.
- Useful as defense-in-depth alongside Option A.

---

## Recommendation

Option A is the only fix that makes the "non-bypassable per-user governance"
claim truthful for untrusted apps in a multi-user deployment.

Option C is worth implementing regardless (defense-in-depth), but it does not
close the vulnerability on its own.

Option B is acceptable only as a temporary, documented position while A is
being built.

---

## References

- `core/src/worker_manager.rs:138` -- worker keyed by app_id only
- `core/src/worker.rs:191` -- context_states map (token -> identity)
- `core/src/worker.rs:286-354` -- token minting and dispatch
- `core/src/worker.rs:589-592` -- SqlQuery token lookup (no binding check)
- `core/src/sql_proxy.rs:50-55` -- token generation (256-bit random)
- `docs/governance-model.md:15-17, 41` -- claims that this violates

---

## Resolution & Remaining Follow-ups (2026-05-31)

**Implemented (Option A):** `context_token` removed from the entire IPC protocol;
`ContextState` identity is fixed at worker spawn and never read from a worker
message. Workers keyed by `(app_id, identity_key)`; one process per principal.
onStart/BYPASSRLS gated to a dedicated system/lifecycle worker via a `run_onstart`
Discover flag. Independent security review verdict: closes the vulnerability,
fail-closed holds, mergeable.

**Remaining follow-ups (none block the security fix; do BEFORE relying on it for
public endpoints / at scale):**

1. **[priority] Anonymous/public RPC + NULL-owner standard webhooks collapse to
   the system identity.** `from_caller(None)` / empty user_id → `ContextState::default()`
   → `is_system()` → the `·system` lifecycle worker. NOT an RLS bypass (rows are
   denied; `onstart_done` is already true), but it merges untrusted anonymous
   traffic into the privileged onStart process. Fix: give anonymous a distinct
   non-system sentinel key so it never shares the BYPASSRLS lifecycle worker.
   Refs: `core/src/routes/workers.rs` (empty user_id), `core/src/extensions/integrations/routes.rs` (created_by None → caller None), `core/src/worker_manager.rs` (`is_system`/`identity_key`).
2. **DoS / unbounded worker count.** Worker count is now ~N_users × M_apps with no
   idle eviction or per-app cap; `get_or_spawn` spawns unboundedly, so a
   user-churning attacker can fork many Bun processes. Add LRU+TTL idle eviction
   and a per-app worker cap. Ref: `core/src/worker_manager.rs:get_or_spawn`.
3. **Per-app log fan-in.** `subscribe_logs` only taps the lifecycle worker;
   user/agent worker logs are no longer visible to subscribers. Add a per-`app_id`
   `broadcast` channel every worker for that app feeds into.
4. **Stale comment** in `core/src/extensions/integrations/mod.rs:29` still references
   "resolved from the IPC context_token"; update to the bound-identity model.

**Lower-priority / optional:**
5. Cache per-app boot template (credentials + agent_boot_config + supervision) so
   `spawn_for` does not re-query Postgres on every new identity's first spawn
   (first-contact only; workers are long-lived). Invalidate on deploy/secret change.
6. Consider an explicit `enum Principal { System, User, Delegated, Anonymous }` owned
   by `sql_proxy` to replace the `is_system`/`run_onstart`/`ContextState::default()`
   sentinel convention spread across worker/manager/ipc (collapses #1 cleanly).
7. `vestigial pool/secrets params on WorkerManager::start_app` (now uses `self.pool`);
   drop them and update the ~6 call sites.
