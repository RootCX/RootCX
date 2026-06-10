# Migration Guide: v0.19 (Governance Refactor)

## What changed

v0.19 removes direct database access from apps. All SQL now flows through
`ctx.sql()` which the core executes under Row-Level Security. Same SQL, only
the transport changes. Apps can no longer bypass governance.

## Removed APIs (crash at boot if still used)

| Old API | Replacement |
|---------|-------------|
| `ctx.databaseUrl` / `postgres(ctx.databaseUrl)` | `ctx.sql(text, params)` |
| `caller.authToken` | Removed entirely (apps never receive tokens) |
| `syncAllConnectedUsers(caller, x)` | `ctx.selfAction("syncConnectedUsers", { actionName: x })` |

## New APIs

### `ctx.sql(text, params?) -> { columns, rows, rowCount }`

Executes a SQL statement inside a governed transaction (RLS-filtered, scoped
to the calling user's permissions).

```typescript
const result = await ctx.sql("SELECT id, name FROM contacts WHERE org_id = $1", [orgId]);
// result.columns = ["id", "name"]
// result.rows = [["uuid-1", "Alice"], ["uuid-2", "Bob"]]
// result.rowCount = 2
```

- Params are positional: `$1`, `$2`, `$3`, ...
- Rows are arrays of values (not objects) — use `toObjects()` helper below
- INSERT/UPDATE/DELETE without RETURNING: `{ columns: [], rows: [], rowCount: 0 }`
- INSERT/UPDATE/DELETE with RETURNING: same format as SELECT (returned rows)
- Max 1000 rows per query (add LIMIT, paginate if needed)
- 8-second timeout per query

### `ctx.collection(entity)` (unchanged)

Structured CRUD on the app's own schema. Same API as before.

```typescript
const contacts = await ctx.collection("contacts").find({ org_id: orgId });
const one = await ctx.collection("contacts").findOne({ id });
await ctx.collection("contacts").insert({ name: "Alice", org_id: orgId });
await ctx.collection("contacts").update({ id, name: "Bob" });
```

### `ctx.selfAction(action, params)`

Replaces the old HTTP callback pattern for integrations.

```typescript
await ctx.selfAction("syncConnectedUsers", { actionName: "sync" });
await ctx.selfAction("triggerAction", { actionName: "refresh", input: {} });
```

## Helper: `toObjects`

If your code accesses results by column name (`row.name`, `row.id`), add this
helper to reconstruct objects from the raw format:

```typescript
function toObjects(result: { columns: string[]; rows: unknown[][] }) {
  return result.rows.map(row =>
    Object.fromEntries(result.columns.map((col, i) => [col, row[i]]))
  );
}

// Usage:
const contacts = toObjects(await ctx.sql("SELECT * FROM contacts WHERE org = $1", [orgId]));
// contacts = [{id: "uuid-1", name: "Alice", ...}, ...]
// contacts[0].name = "Alice"
```

## Migration steps

### 1. Remove the direct connection

```typescript
// DELETE these lines:
import postgres from "postgres";
let db: any;
// in onStart:
db = postgres(ctx.databaseUrl, { max: 3 });
```

### 2. Convert each query

```typescript
// BEFORE (tagged template):
const users = await db`SELECT * FROM users WHERE org_id = ${orgId} AND status = ${status}`;
// users[0].name = "Alice"

// AFTER (positional params + toObjects):
const users = toObjects(await ctx.sql(
  "SELECT * FROM users WHERE org_id = $1 AND status = $2", [orgId, status]
));
// users[0].name = "Alice"  (same access pattern)
```

### 3. Convert INSERT/UPDATE/DELETE

```typescript
// BEFORE:
await db`INSERT INTO logs (message, level) VALUES (${msg}, ${level})`;
const [created] = await db`INSERT INTO users (name) VALUES (${name}) RETURNING *`;

// AFTER:
await ctx.sql("INSERT INTO logs (message, level) VALUES ($1, $2)", [msg, level]);
const result = await ctx.sql("INSERT INTO users (name) VALUES ($1) RETURNING *", [name]);
const created = toObjects(result)[0];
```

### 4. Thread `ctx` through your functions

The 3rd argument of every RPC/job handler is `ctx`. Pass it to internal
functions instead of `db`:

```typescript
// BEFORE:
async function getUser(db, userId) { return (await db`SELECT...`)[0]; }

// AFTER:
async function getUser(ctx, userId) {
  return toObjects(await ctx.sql("SELECT * FROM users WHERE id = $1", [userId]))[0];
}
```

### 5. Remove the postgres package

```bash
# Remove "postgres" from package.json dependencies, then:
rm -rf node_modules bun.lock
```

## Constraints

| Constraint | Reason |
|-----------|--------|
| No DDL (`CREATE TABLE`, `ALTER`, `DROP`, `CREATE INDEX`) | Core manages the schema at deploy time |
| No `SET`, `RESET`, `DO $$` | Blocked by the SQL proxy |
| No multi-statement (`;` between statements) | Single statement per call (Postgres extended protocol) |
| Max 1000 rows per query | Add LIMIT; paginate for larger sets |
| 8-second timeout | Optimize or split long queries |
| No `set_config()` calls | Revoked from the app role |

## Verification

After migration, run:

```bash
grep -rE "databaseUrl|postgres\(|authToken|syncAllConnectedUsers" backend/ --include="*.ts"
```

Must return nothing (ignore `node_modules/`).

## API response changes

### Webhook list (`GET /api/v1/apps/{id}/webhooks`)

The response no longer includes the full `token`. It returns `prefix` (first 8
chars) for display/identification only. The full token is shown once at creation
and never retrievable again.

```diff
- { "name": "stripe", "token": "a1b2c3d4e5f6...full64chars" }
+ { "name": "stripe", "prefix": "a1b2c3d4" }
```

If your UI copies the webhook URL for users, it must now store the token at
creation time or prompt the user to regenerate.

### New: Service accounts API

New endpoints under `/api/v1/service-accounts` for M2M automation:

| Endpoint | Purpose |
|----------|---------|
| `POST /api/v1/service-accounts` | Create SA (requires `admin:sa.manage`) |
| `POST .../credentials` | Generate `rcs_` credential (shown once) |
| `POST /api/v1/auth/token` | Client-credentials token exchange (RFC 6749 s4.4) |
| `POST .../disable` / `POST .../enable` | Kill switch |
| `POST .../transfer-ownership` | Reassign owner of record |
| `POST .../act-as` / `DELETE .../act-as` | Manage act-as delegation grants |

These are new endpoints (no migration needed), but if your website provisions
automation accounts, use this API instead of creating human users.

---

## Behavioral changes (exhaustive)

Every change below was verified against the codebase. Admins (role with `*`
permission) are unaffected; all breaking changes apply to non-admin users
and apps.

### Permission gates added (previously open to any authenticated user)

| Action | Before (v0.18) | After (v0.19) | Required permission |
|--------|---------------|---------------|---------------------|
| Install app | Any user | Admin only (except first-boot) | `admin:apps.install` |
| Uninstall app | Any user | Admin only | `admin:apps.install` |
| Deploy backend | Any user | Admin only | `admin:apps.deploy` |
| Deploy frontend | Any user | Admin only | `admin:apps.deploy` |
| Call an app RPC | Any user | Requires invoke permission | `app:{id}:invoke` |
| View system schema structure | Any user | Admin only | `admin:db.query` |
| Execute admin SQL | Any user | Admin only + read-only enforced | `admin:db.query` |
| Manage app secrets | Any user | Admin only | `admin:secrets.manage` |
| Manage platform secrets | System-user check only | Admin only | `admin:secrets.manage` |
| List/view MCP servers | Any user | Admin only | `admin:mcp.manage` |
| Manage agent config | No check | Admin only | `admin:agents.manage` |

### Data access behavior changes

| Scenario | Before (v0.18) | After (v0.19) |
|----------|---------------|---------------|
| User reads data without permission | HTTP 403 error | 200 with 0 rows (RLS silent filter) |
| User writes data without permission | HTTP 403 error | Postgres error (RLS WITH CHECK) |
| Cross-app linked/federated query | Silently skipped by Rust | 0 rows for unauthorized schemas (RLS) |
| Share-token RPC reads data | Had full DB access via app | Read-only delegation of the share creator: visible rows = what the creator may read, `.read` permissions only |
| Anonymous RPC (no token) reads data | Had full DB access via app | ctx.sql returns 0 rows (no identity = deny-all) |

### App sandbox (breaking for app code)

| Capability | Before (v0.18) | After (v0.19) |
|-----------|---------------|---------------|
| Direct DB connection | App receives `DATABASE_URL` in Discover IPC | Removed from IPC entirely; use `ctx.sql()` |
| User JWT token | App receives `caller.authToken` | Removed; never exposed |
| Direct TCP to Postgres | Possible (app had credentials) | Blocked (pg_hba + no credentials) |
| DDL in app code (CREATE INDEX, etc.) | Allowed | Blocked (restricted role, no DDL) |
| Read system tables | Possible | Blocked (REVOKE on rootcx_system/pgmq/cron) |
| SET/RESET/DO commands in SQL | Possible | Blocked (validate_sql + set_config revoked) |

### Automated job behavior changes

| Scenario | Before (v0.18) | After (v0.19) |
|----------|---------------|---------------|
| Cron agent job without owner (created_by NULL) | Falls back to SYSTEM_USER (runs as admin) | Refused (deny-by-default) |
| Regular job without owner | Falls back to SYSTEM_USER (runs as admin) | Refused (deny-by-default) |
| Channel message (Slack/Telegram) | No delegation check | Requires valid delegation; revoked = agent mute |
| Webhook agent without delegation | Already checked | Still checked (unchanged) |

### Auth token delivery (magic-link / OIDC callback)

v0.19 introduces nonce-exchange token delivery. The callback redirect carries a
single-use `auth_nonce`; the SDK exchanges it server-side for the tokens, so the
tokens themselves never land in the URL (no proxy/server logs, no browser
history, no `Referer`).

| Client | Delivery | Action needed |
|--------|----------|---------------|
| New projects (SDK >= 0.19) | `token_delivery=nonce`: tokens via nonce exchange only, never in the URL | None. Secure by default, no flag. |
| Existing projects (older SDK) | Tokens still delivered in the URL (query + fragment), exactly as before v0.19 | None. Backwards compatible, keeps working unchanged. |

There is no flag and no opt-in: the server keys the behavior off the request the
SDK sends. A project built on SDK >= 0.19 gets nonce delivery automatically; an
older SDK keeps receiving URL tokens. Nothing breaks on upgrade.

Security note for existing projects: URL token delivery exposes the tokens to
proxy/server access logs, browser history, and `Referer`. This is an accepted,
backwards-compatibility tradeoff for clients that predate nonce exchange.
Upgrading the client SDK to >= 0.19 closes it with no code change on your side.

### Unchanged behaviors (no migration needed)

- Agent invoke gate (`app:{id}:invoke`) was already enforced
- Cron creation requires `app:{id}:cron.write` (unchanged)
- Hook creation open to authenticated users (unchanged)
- Integration actions require `integration:{id}:{action}` (unchanged)
- Worker start/stop requires admin (unchanged)
- Agent intersection logic (unchanged, now enforced by RLS instead of Rust)

### Agent registration

`register_agent()` now accepts an optional `deployer: Uuid` (the user who
deployed the app). The deployer becomes the agent's owner of record. If omitted,
the system backfills ownership from the app installer.

This is internal (no app-facing API change), but if your provisioning code calls
the core's deploy endpoint, the deploying user's identity is now captured and
used for governance (fire-gate, delegation).

### What to do after upgrading the core

1. **Assign roles**: ensure non-admin users have roles with the appropriate
   permissions (`app:{id}:invoke` for each app they should access, entity
   read/write permissions for data access).
2. **Check crons/hooks**: any cron or hook with `created_by = NULL` will stop
   executing. Re-create them with an authenticated user or backfill the owner.
3. **Channel links**: users must have a valid delegation to the agent. Linking
   via `/link` creates it automatically; existing links from before v0.19 need
   a one-time re-link or manual delegation backfill.

---

## Why this change

The direct DB connection gave apps a master password to the database. Any app
could read any user's data, bypass permissions, or access other apps' schemas.
The new model enforces governance at the database level (RLS): an app can only
see what the current user is allowed to see, enforced by PostgreSQL itself.
This is not optional or bypassable — it is the architecture.
