# Agent Identity & Authority -- Final Implementation Plan

Repo: `rootCX2` (Rust Core). Scope: Core-only. Ships as ONE release, no phasing.
Goal: make AI agents first-class, governed principals for multi-user enterprise.
All line refs verified against current code (2026-05-27, commit ebf84db).

## The feature in one paragraph

An agent never "becomes" a user. It is its own principal (`agent_uid`) with a fixed
**capability grant** (what it may ever touch, set by its creator/admin). Every time it acts it
runs under a **delegation** from an accountable human, and its effective authority is
`grant(agent) ∩ perms(delegator)` -- it can never exceed either side. The human is the live
invoker, or, for autonomous runs (cron, DB-row trigger, webhook), the human who created
that trigger (a "standing mandate"). Who may invoke an agent is itself gated (invocation ACL).
Every mutation is audited as "agent X acting for human Y via trigger Z."

No delegation context = **DENY**. No exceptions. No fallthrough to agent's full grant.

## Compliance references

- **RFC 8693** (Token Exchange): `act` claim semantics, delegation vs impersonation
- **industry zero-trust Zero Trust for AI Agents**: effective perms = intersection, agent auth-unaware, policy engine decides
- **Okta IGA**: agent = first-class identity, least-privilege, instant revocation, lifecycle governance
- **RFC 8707**: audience-bound tokens (aud claim)
- **Scalekit**: 5-15min TTL, deny-by-default, structured audit (performed_by + on_behalf_of)

## How authority is consumed today (fact-checked map)

```
PATH 1 -- agent tools query_data / mutate_data       <- the dominant data path
  worker.rs:485  agent_uid = agent_user_id(&aid)
  worker.rs:486  permissions = resolve_permissions(&pool, agent_uid)   <- delegator IGNORED
  worker.rs:418  invoker_uid = invoker_user_ids.get(&invoke_id).copied()  <- in scope, unused
        |
        v  tool_executor.rs:30  check_permission(&permissions, "tool:{name}")   <- first gate
        v  tool_executor.rs:38  ToolContext { permissions, invoker_user_id, ... }
        v  query_data.rs:49    check_permission(&ctx.permissions, "app:{app}:{entity}.read")  <- second gate
        v  SQL runs directly on ctx.pool
  -> today: any user invoking an agent gets the agent's FULL permission set (which is '*' admin).

  WHY '*': sync_agent_rbac (agents/mod.rs:160-180) assigns every agent the 'admin' role.
  So resolve_permissions(agent_uid) always returns ["*"]. The intersection fix is correct
  regardless because has_permission("*", anything) = true.

PATH 2 -- actions call_action -> app worker -> Core REST callback
  call_action.rs:52  effective_uid = ctx.invoker_user_id.unwrap_or(ctx.user_id)
  call_action.rs:53  caller.call(app, action, input, effective_uid)
  worker_manager.rs:310-313  RpcCaller { user_id, email: "", auth_token: None }  <- THE 401 BUG
        |
        v  app worker calls Core REST /collections -> routes/crud.rs:306
        v  crud.rs resolves permissions from identity.user_id (JWT sub claim)
  -> today: 401, because no token is forwarded.

PATH 3 -- X-Run-As header
  extensions/integrations/routes.rs:110  reads x-run-as, admin-gates at :105 via resolve_permissions
  -> Footgun for admins, not a hole. Unchanged.
```

## The enforcement design (all decisions resolved)

One rule, computed server-side. Token is **identity-only** (`sub`=delegator, `act`=agent,
`aud`='rootcx-core', TTL 120s). No capabilities in the token -- HS256 symmetric
(auth/mod.rs:34-36, jwt.rs:53) makes capability-bearing tokens forgeable if secret leaks.

### Path 1 fix (tool calls)

At `worker.rs:486`, replace:
```rust
let permissions = resolve_permissions(&pool, agent_uid).await.map(|(_, p)| p).unwrap_or_default();
```
With:
```rust
let agent_perms = resolve_permissions(&pool, agent_uid).await.map(|(_, p)| p).unwrap_or_default();
let permissions = match invoker_uid {
    Some(uid) => {
        let (_, invoker_perms) = resolve_permissions(&pool, uid).await.unwrap_or_default();
        intersect_permissions(&agent_perms, &invoker_perms)
    }
    None => vec![], // DENY -- no delegation context = no authority
};
```

This flows into `tool_executor.rs:30` (tool gate) AND into each tool's internal
`check_permission` (e.g. query_data.rs:49). Both gates enforce the intersection
without any change to tool code.

### Path 2 fix (action callbacks)

`AppActionCallImpl` (worker_manager.rs:301-303) gets `pool: PgPool` + `auth: Arc<AuthConfig>`.
In `call()`:
```rust
let token = jwt::mint_delegated(&self.auth, user_id, agent_uid)?;
RpcCaller { user_id: user_id.to_string(), email: String::new(), auth_token: Some(token) }
```

`routes/crud.rs:306` becomes act-aware:
```rust
let (_, perms) = if let Some(ref actor) = identity.actor {
    let agent_uid: Uuid = actor.sub.parse()?;
    let (_, agent_perms) = resolve_permissions(&pool, agent_uid).await?;
    let (_, delegator_perms) = resolve_permissions(&pool, identity.user_id).await?;
    (vec![], intersect_permissions(&agent_perms, &delegator_perms))
} else {
    resolve_permissions(&pool, identity.user_id).await?
};
```

### Autonomy fix (cron, hooks, webhooks)

All three autonomous dispatch paths carry `created_by` as `invoker_user_id`.
Before dispatch, validate via `delegations` table (standing mandate check):

```rust
let valid = sqlx::query_scalar::<_, bool>(
    "SELECT EXISTS(SELECT 1 FROM rootcx_system.delegations
     WHERE delegator_uid = $1 AND agent_uid = $2
     AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at > now()))"
).bind(delegator_uid).bind(agent_uid).fetch_one(pool).await?;
if !valid { return Err("no valid standing mandate"); }
```

## Security rules (RFC 8693 / industry zero-trust / Okta-IGA compliant)

1. Effective authority = `intersect_permissions(grant, delegator)`, server-side, never in token.
2. Identity-only token: `sub`=delegator, `act`={sub: agent_uid, act: null}, `aud`='rootcx-core', TTL 120s.
3. **No delegation context = DENY.** `invoker_uid = None` -> empty permissions -> all checks fail.
4. Autonomous runs require a valid standing mandate (`delegations` table, checked live at dispatch).
5. `sub`/`act` come ONLY from verified JWT; `X-Run-As` stays admin-only.
6. Audit captures `actor_uid` + `delegator_uid` + `trigger_ref` on EVERY mutation (PG session vars).
7. Invocation ACL: `check_permission(perms, "app:{id}:invoke")` gates who may invoke each agent.

## Concrete changes per file (verified refs, no phasing)

### New: `core/src/extensions/rbac/policy.rs`

```rust
/// Compute the intersection of two permission sets.
/// A permission P is in the result IFF both sides grant it.
/// Handles wildcards: '*' grants everything, 'ns:scope:*' grants the subtree.
pub fn intersect_permissions(a: &[String], b: &[String]) -> Vec<String> {
    if a.iter().any(|p| p == "*") { return b.to_vec(); }
    if b.iter().any(|p| p == "*") { return a.to_vec(); }
    // Keep permissions from A that B also grants, and vice versa (union of both directions)
    let mut result: Vec<String> = a.iter()
        .filter(|p| has_permission(b, p))
        .cloned()
        .collect();
    for p in b {
        if has_permission(a, p) && !result.contains(p) {
            result.push(p.clone());
        }
    }
    result.sort_unstable();
    result.dedup();
    result
}
```

### Modified: `core/src/auth/jwt.rs`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActorClaim {
    pub sub: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub act: Option<Box<ActorClaim>>,  // RFC 8693 nestable
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    #[serde(default)]
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub act: Option<ActorClaim>,       // RFC 8693 actor claim
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aud: Option<String>,           // RFC 8707 audience
    pub exp: i64,
    pub iat: i64,
}

pub fn mint_delegated(config: &AuthConfig, delegator_uid: Uuid, agent_uid: Uuid) -> Result<String, RuntimeError> {
    let now = chrono::Utc::now().timestamp();
    let claims = Claims {
        sub: delegator_uid.to_string(),
        email: String::new(),
        session_id: None,
        act: Some(ActorClaim { sub: agent_uid.to_string(), act: None }),
        aud: Some("rootcx-core".into()),
        exp: now + 120, // 120s TTL -- proportional to single sync REST call
        iat: now,
    };
    jsonwebtoken::encode(&Header::default(), &claims, &config.encoding_key)
        .map_err(|e| RuntimeError::Auth(e.to_string()))
}
```

### Modified: `core/src/auth/identity.rs`

The `Identity` extractor must parse `act` from the token:
- Add `pub actor: Option<ActorClaim>` to `Identity` struct
- On decode: `identity.actor = claims.act`
- Backward compat: tokens without `act` -> `actor = None` (existing behavior)

### Modified: `core/src/worker.rs:486-487`

Replace permissions resolution with intersection + deny-on-None (see Path 1 fix above).

### Modified: `core/src/tool_executor.rs`

No change needed -- it already receives `permissions` from worker.rs and passes to ToolContext.
The intersection is applied upstream.

### Modified: `core/src/worker_manager.rs:301-322`

```rust
struct AppActionCallImpl {
    wm: Arc<WorkerManager>,
    pool: PgPool,          // NEW
    auth: Arc<AuthConfig>, // NEW
}
```
In `call()`: mint delegated token, pass in RpcCaller.

### Modified: `core/src/routes/crud.rs:306`

Act-aware authorization (see Path 2 fix above). Legacy tokens (no `act`) unchanged.

### Modified: `core/src/scheduler.rs:50` (cron agent dispatch)

```rust
// Replace: invoker_user_id: None,
// With:
invoker_user_id: job_msg.user_id, // from crons.created_by via enqueue_cron
```

Also add delegation validation before dispatch:
```rust
if let Some(delegator) = job_msg.user_id {
    let agent_uid = agents::agent_user_id(&target_app);
    if !delegations::is_valid(&pool, delegator, agent_uid).await? {
        warn!(msg_id, "cron agent: no valid delegation");
        jobs::fail(&pool, msg_id).await?;
        continue;
    }
}
```

### Modified: `core/src/extensions/hooks.rs`

1. Add `created_by UUID` column to `entity_hooks` table (ALTER TABLE + bootstrap DDL)
2. `create_hook` handler: capture `identity.user_id` as `created_by`
3. PG trigger function `hooks_trigger_fn`: include `created_by` in the pgmq message:
   ```sql
   SELECT created_by INTO v_created_by FROM rootcx_system.entity_hooks WHERE id = hook.id;
   -- add to jsonb: 'user_id', v_created_by
   ```
4. Scheduler hook dispatch (scheduler.rs:110-123): read `user_id` from job message, pass as `invoker_user_id`
5. Add delegation validation (same pattern as cron)

### Modified: `core/src/webhooks.rs`

1. Add `created_by UUID` column to webhooks table
2. `sync_webhooks`: accept `created_by` param, store it
3. NEW route `POST /api/v1/webhooks/incoming/{token}`:
   ```rust
   async fn receive_webhook(Path(token): Path<String>, State(rt): State<SharedRuntime>, body: Bytes) -> ... {
       let wh = webhooks::lookup_token(&pool, &token).await?.ok_or(404)?;
       let delegator = wh.created_by.ok_or("webhook has no owner")?;
       let agent_uid = agents::agent_user_id(&wh.app_id);
       if !delegations::is_valid(&pool, delegator, agent_uid).await? {
           return Err(403);
       }
       // Enqueue job with user_id = delegator
       jobs::enqueue(&pool, &wh.app_id, payload, Some(delegator)).await?;
       scheduler_wake.notify_one();
   }
   ```

### New: `core/src/delegations.rs`

```rust
pub async fn bootstrap(pool: &PgPool) -> Result<(), RuntimeError> {
    sqlx::query(r#"
        CREATE TABLE IF NOT EXISTS rootcx_system.delegations (
            id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            delegator_uid UUID NOT NULL REFERENCES rootcx_system.users(id),
            agent_uid     UUID NOT NULL,
            trigger_type  TEXT NOT NULL CHECK (trigger_type IN ('cron', 'hook', 'webhook', 'manual')),
            trigger_ref   UUID,          -- FK to the cron/hook/webhook ID
            created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
            expires_at    TIMESTAMPTZ,   -- NULL = never expires
            revoked_at    TIMESTAMPTZ    -- NULL = active
        )
    "#).execute(pool).await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_delegations_lookup \
         ON rootcx_system.delegations (delegator_uid, agent_uid) WHERE revoked_at IS NULL"
    ).execute(pool).await?;
    Ok(())
}

pub async fn is_valid(pool: &PgPool, delegator: Uuid, agent: Uuid) -> Result<bool, RuntimeError> {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM rootcx_system.delegations
         WHERE delegator_uid = $1 AND agent_uid = $2
         AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at > now()))"
    ).bind(delegator).bind(agent).fetch_one(pool).await.map_err(RuntimeError::Schema)
}

pub async fn revoke(pool: &PgPool, delegation_id: Uuid) -> Result<(), RuntimeError> {
    sqlx::query("UPDATE rootcx_system.delegations SET revoked_at = now() WHERE id = $1")
        .bind(delegation_id).execute(pool).await.map_err(RuntimeError::Schema)?;
    Ok(())
}
```

### Modified: `core/src/extensions/agents/routes.rs:152`

Add invocation ACL before dispatch:
```rust
pub async fn invoke_agent(identity: Identity, ...) -> ... {
    let pool = rt.pool().clone();
    let (_, perms) = resolve_permissions(&pool, identity.user_id).await?;
    check_app_perm(&perms, &app_id, "invoke")?; // "app:{id}:invoke"
    // ... rest of existing code
}
```

### Modified: `core/src/extensions/audit.rs`

1. Add columns to `audit_log`:
   ```sql
   ALTER TABLE rootcx_system.audit_log ADD COLUMN IF NOT EXISTS actor_uid UUID;
   ALTER TABLE rootcx_system.audit_log ADD COLUMN IF NOT EXISTS delegator_uid UUID;
   ALTER TABLE rootcx_system.audit_log ADD COLUMN IF NOT EXISTS trigger_ref TEXT;
   ```

2. Update `audit_trigger_fn` to read PG session vars:
   ```sql
   CREATE OR REPLACE FUNCTION rootcx_system.audit_trigger_fn()
   RETURNS TRIGGER AS $$
   DECLARE rec_id TEXT; v_actor UUID; v_delegator UUID; v_trigger TEXT;
   BEGIN
       rec_id := CASE WHEN TG_OP = 'DELETE' THEN OLD.id::TEXT ELSE NEW.id::TEXT END;
       v_actor := nullif(current_setting('rootcx.actor_uid', true), '')::UUID;
       v_delegator := nullif(current_setting('rootcx.delegator_uid', true), '')::UUID;
       v_trigger := nullif(current_setting('rootcx.trigger_ref', true), '');
       INSERT INTO rootcx_system.audit_log
           (table_schema, table_name, record_id, operation, old_record, new_record, actor_uid, delegator_uid, trigger_ref)
       VALUES (
           TG_TABLE_SCHEMA, TG_TABLE_NAME, rec_id, TG_OP,
           CASE WHEN TG_OP IN ('UPDATE','DELETE') THEN to_jsonb(OLD) ELSE NULL END,
           CASE WHEN TG_OP IN ('INSERT','UPDATE') THEN to_jsonb(NEW) ELSE NULL END,
           v_actor, v_delegator, v_trigger
       );
       RETURN CASE WHEN TG_OP = 'DELETE' THEN OLD ELSE NEW END;
   END;
   $$ LANGUAGE plpgsql SECURITY DEFINER
   ```

3. Rust-side: helper fn to SET LOCAL before write transactions:
   ```rust
   pub async fn set_audit_context(tx: &mut PgTransaction, actor: Uuid, delegator: Option<Uuid>, trigger: Option<&str>) {
       let _ = sqlx::query(&format!("SET LOCAL rootcx.actor_uid = '{actor}'")).execute(&mut **tx).await;
       if let Some(d) = delegator {
           let _ = sqlx::query(&format!("SET LOCAL rootcx.delegator_uid = '{d}'")).execute(&mut **tx).await;
       }
       if let Some(t) = trigger {
           let _ = sqlx::query(&format!("SET LOCAL rootcx.trigger_ref = '{}'", t.replace('\'', "''"))).execute(&mut **tx).await;
       }
   }
   ```

   Called in: `tools/mutate_data.rs` (agent writes), `routes/crud.rs` (API writes), webhook/cron dispatch paths.

## Critical corrections from independent review (Claude subagent)

### Fix 1: dispatch_agent_job signature must change

The plan references "scheduler.rs:50" but `dispatch_agent_job` takes `(pool, wm, msg_id, target_app, message, label)` -- it has no access to `user_id`. Fix: add `invoker_user_id: Option<Uuid>` parameter. Both call sites (line 122 for hooks, line 140 for crons) pass `job_msg.user_id` through. Inside the fn, set `payload.invoker_user_id = invoker_user_id`.

### Fix 2: Resolve permissions ONCE per invoke, not per tool call

`resolve_permissions` hits the DB (2 queries). The plan puts intersection at worker.rs:486 which fires per tool call. For a 50-tool-call session, that's 200 queries. Fix: resolve both permission sets ONCE at invoke start (when `AgentInvoke` arrives), store the intersected result in the `invoker_user_ids` equivalent map (or a new `effective_permissions` map keyed by invoke_id). Pass the cached vec into each tool spawn. Invoker perms don't change mid-session.

### Fix 3: Auto-create delegations on trigger creation

When a user creates a cron (`crons::create`), hook (`create_hook`), or webhook (during `sync_webhooks` with `created_by`), automatically INSERT a delegation row:
```rust
delegations::create(&pool, created_by, agent_uid, trigger_type, trigger_id).await?;
```
This eliminates the chicken-and-egg: creating a trigger = granting the standing mandate.

### Fix 4: Migration for existing deployments

On bootstrap (schema.rs or delegations::bootstrap), run a one-time migration:
```sql
INSERT INTO rootcx_system.delegations (delegator_uid, agent_uid, trigger_type, trigger_ref)
SELECT cs.created_by, -- delegator
       uuid_generate_v5('9a3b4c5d-6e7f-4001-8293-a4b5c6d7e8f9', 'agent:' || cs.app_id), -- agent_uid
       'cron', cs.id
FROM rootcx_system.cron_schedules cs
WHERE cs.created_by IS NOT NULL
ON CONFLICT DO NOTHING;
-- Same pattern for entity_hooks (once created_by column exists)
```
Crons/hooks without `created_by` (legacy, no owner) remain denied until an admin claims them. This is intentional: deny-by-default for unowned autonomous triggers.

### Fix 5: mutate_data.rs DELETE uses bare pool.execute() -- wrap in tx for SET LOCAL

Line 111 does `sqlx::query(&sql).bind(uuid).execute(&ctx.pool)` -- no transaction. SET LOCAL has no effect outside a tx. Fix: wrap each mutate_data write operation in `pool.begin()` / `tx.commit()` with `set_audit_context` before the write. Same for `bulk_insert` (line 104).

### Fix 6: Clarify token lifecycle for Path 2

The delegated token is minted ONCE per `call_action` invocation (not per session). If an action fans out to multiple Core API calls, the app worker already holds the token (120s is sufficient for any realistic fan-out within a single action execution). If a single action truly takes > 120s of sequential Core API calls, it's architecturally broken regardless.

## Open items (RESOLVED by code inspection)

| Original open item | Resolution |
|---|---|
| `routes/crud.rs` exact authz | **Confirmed**: resolves perms from `identity.user_id` via `resolve_permissions(&pool, identity.user_id).await` at line 306. Intersection logic adds a branch when `identity.actor` is Some. |
| Whether webhooks have dispatch handler | **Confirmed NO.** Only bootstrap, sync, list, lookup_token. Dispatch is net-new (~60 lines). |
| Cross-tenant isolation of agent user row | **Non-issue.** Architecture = 1 DB per tenant. The UUID v5 hash is deterministic per app_id within a single DB. No cross-tenant path exists. |

## Tests (35 code paths, 9 CRITICAL -- all required)

### Unit tests: `core/src/extensions/rbac/policy.rs`

```
intersect_permissions:
  1. both concrete, overlapping subset -> intersection
  2. both concrete, no overlap -> empty vec (CRITICAL: must not fallthrough)
  3. one side has '*' -> returns the other side verbatim
  4. both have '*' -> returns ["*"]
  5. 'app:X:*' vs 'app:X:entity.read' -> keeps 'app:X:entity.read'
  6. 'app:X:*' vs 'app:Y:entity.read' -> empty (different namespace)
  7. both empty -> empty
  8. one empty, one has perms -> empty (CRITICAL)
```

### Unit tests: `core/src/auth/jwt.rs`

```
  9.  mint_delegated roundtrip: encode -> decode -> sub=delegator, act.sub=agent
  10. decode token WITHOUT act -> claims.act = None (CRITICAL backward compat)
  11. nested act: A delegates to B delegates to C -> nested structure preserved
  12. expired delegated token -> decode fails
  13. aud claim present and correct
```

### Integration tests: worker.rs intersection

```
  14. low-priv invoker + admin agent -> effective = invoker's perms only (CRITICAL)
  15. admin invoker + restricted agent -> effective = agent's perms only
  16. invoker_uid = None -> permissions = empty -> tool call denied (CRITICAL)
  17. invoker_uid = Some(user) with matching perms -> tool call succeeds
  18. tool gate (tool:{name}) also respects intersection
```

### Integration tests: worker_manager.rs (Path 2 / 401 bug)

```
  19. agent action carries valid delegated token -> Core REST responds 200 (CRITICAL regression)
  20. delegated token has correct sub=delegator, act.sub=agent
  21. delegated token has aud='rootcx-core' and TTL <= 120s
```

### Integration tests: routes/crud.rs act-aware

```
  22. token with act=agent -> authorizes against intersection
  23. token without act -> existing behavior unchanged (CRITICAL regression)
  24. delegator perms revoked in DB after token mint -> denied on next call (live revocation)
```

### Integration tests: scheduler autonomy

```
  25. cron dispatch: job_msg.user_id becomes invoker_user_id
  26. cron dispatch: no valid delegation -> job fails with warning (CRITICAL)
  27. hook dispatch: created_by flows through pgmq -> invoker_user_id
  28. hook dispatch: no valid delegation -> denied (CRITICAL)
```

### Integration tests: webhook dispatch (net-new)

```
  29. POST /incoming/{valid_token} with valid delegation -> job enqueued
  30. POST /incoming/{invalid_token} -> 404
  31. POST /incoming/{valid_token} with revoked delegation -> 403 (CRITICAL)
  32. webhook.created_by = None -> denied
```

### Integration tests: invocation ACL

```
  33. user WITHOUT 'app:{id}:invoke' permission -> 403 (CRITICAL)
  34. user WITH 'app:{id}:invoke' -> invoke proceeds
  35. user with 'app:{id}:*' wildcard -> invoke proceeds (wildcard coverage)
```

### Integration tests: audit

```
  36. SET LOCAL rootcx.actor_uid -> captured in audit_log row
  37. SET LOCAL rootcx.delegator_uid -> captured in audit_log row
  38. no SET LOCAL (legacy/direct) -> actor_uid = NULL (backward compat)
```

## NOT in scope

- Asymmetric JWT signing (RS256/ES256) -- requires key distribution infra, defer to security hardening sprint
- DPoP / sender-constrained tokens -- requires client-side key management, defer
- Per-agent capability-grant UI in Studio -- Studio change, not Core
- Sub-agent chaining beyond depth-1 -- the guard at worker.rs:415 stays; nestable `act` claim is READY for when it's lifted
- Admin governance UI (inventory, revocation dashboard) -- Studio/frontend, not Core

## What already exists (reuse, don't rebuild)

| Component | Exists at | Reuse? |
|---|---|---|
| Agent UID generation | `agents/mod.rs:26` agent_user_id() | Yes, as-is |
| Invoker plumbing | `AgentInvokePayload.invoker_user_id`, `ToolContext.invoker_user_id` | Yes, already wired end-to-end |
| Permission resolution | `rbac/policy.rs:65` resolve_permissions() | Yes, call twice (agent + invoker) |
| Wildcard matching | `rbac/policy.rs:114` has_permission() | Yes, used inside intersect_permissions |
| Cron created_by | `crons.rs:45` column + `enqueue_cron` PG fn reads it | Yes, just plumb into agent dispatch |
| JWT encode/decode | `jwt.rs:19-57` | Extend with act/aud fields |
| PG audit trigger | `audit.rs:57-73` | Extend with session var reads |
| Sub-agent depth guard | `worker.rs:415` | Keep as-is, don't contradict |

## Data flow diagram

```
                    INTERACTIVE INVOKE
                    ==================
User (browser/API)
  |
  |  POST /api/v1/apps/{id}/agent/invoke
  |  JWT: sub=user_id
  v
invoke_agent (agents/routes.rs)
  |-- check_permission(perms, "app:{id}:invoke")    <-- INVOCATION ACL
  |-- payload.invoker_user_id = Some(identity.user_id)
  v
worker.rs (tool execution)
  |-- agent_perms = resolve_permissions(agent_uid)
  |-- invoker_perms = resolve_permissions(invoker_uid)
  |-- ctx.permissions = intersect(agent_perms, invoker_perms)
  |                         |
  |    invoker=None ------> | -> permissions = [] -> ALL tools denied
  v
tool_executor.rs
  |-- check_permission(ctx.permissions, "tool:{name}")
  v
query_data / mutate_data / call_action
  |-- check_permission(ctx.permissions, "app:{app}:{entity}.{op}")
  |-- [mutate_data] SET LOCAL rootcx.actor_uid/delegator_uid before tx
  v
SQL executes, audit trigger captures actor context


                    AUTONOMOUS DISPATCH (cron/hook/webhook)
                    =======================================
pg_cron / entity hook trigger / external HTTP
  |
  |  enqueue job to pgmq with user_id = created_by
  v
scheduler.rs
  |-- read job_msg.user_id (= created_by = delegator)
  |-- delegations::is_valid(delegator, agent_uid)?
  |       |
  |       NO  --> job fails, warn log
  |       YES
  |-- AgentInvokePayload { invoker_user_id: Some(delegator), ... }
  v
worker.rs (same path as interactive)
  |-- intersect(agent_perms, delegator_perms)
  v
tool execution with bounded authority


                    PATH 2 -- ACTION CALLBACK
                    =========================
Agent calls call_action tool
  |
  |-- effective_uid = invoker_user_id.unwrap_or(agent_uid)
  |-- ActionCaller::call(app, action, input, effective_uid)
  v
AppActionCallImpl
  |-- token = mint_delegated(auth, effective_uid, agent_uid)
  |-- RpcCaller { user_id, email, auth_token: Some(token) }
  v
App worker calls Core REST (/collections/...)
  |-- JWT: sub=delegator, act={sub:agent_uid}, aud='rootcx-core', exp=now+120s
  v
routes/crud.rs
  |-- identity.actor is Some -> act-aware path
  |-- agent_perms = resolve_permissions(agent_uid)
  |-- delegator_perms = resolve_permissions(delegator_uid)
  |-- effective = intersect(agent_perms, delegator_perms)
  |-- check_app_perm(&effective, ...)
  v
SQL executes with full audit context
```

## Provenance

- Design: prior session (CEO + eng review + independent challenge)
- This version: /plan-eng-review against live code, all line refs fact-checked
- Decisions D1-D12 recorded interactively (intersection algorithm, deny-on-None,
  nestable act, SET LOCAL audit, webhook dispatch, cron/hook fix, DB-live authz,
  delegation gate, RBAC invocation ACL, token TTL/aud, DRY fn, full test coverage)
- Compliance validated against: RFC 8693, RFC 8707, industry zero-trust Zero Trust for AI Agents,
  Okta IGA, Scalekit, Arcade, Google Cloud IAM, GitGuardian, Gravitee, Strata

## GSTACK REVIEW REPORT

| Review | Trigger | Why | Runs | Status | Findings |
|--------|---------|-----|------|--------|----------|
| CEO Review | `/plan-ceo-review` | Scope & strategy | 0 | -- | -- |
| Codex Review | `/codex review` | Independent 2nd opinion | 0 | -- | -- |
| Eng Review | `/plan-eng-review` | Architecture & tests (required) | 1 | CLEAR | 12 issues resolved, 0 critical gaps |
| Design Review | `/plan-design-review` | UI/UX gaps | 0 | -- | -- |
| DX Review | `/plan-devex-review` | Developer experience gaps | 0 | -- | -- |
| Outside Voice | Claude subagent | Independent challenge | 1 | CLEAR | 7 findings, 6 incorporated, 1 doc-only |

- **CROSS-MODEL:** Outside voice found 4 critical issues (fn signature, perf, migration, tx blocks) that the primary review missed. All incorporated into the plan.
- **UNRESOLVED:** 0
- **VERDICT:** ENG + OUTSIDE VOICE CLEARED -- all issues resolved. Ready to implement.
