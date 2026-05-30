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

## Why this change

The direct DB connection gave apps a master password to the database. Any app
could read any user's data, bypass permissions, or access other apps' schemas.
The new model enforces governance at the database level (RLS): an app can only
see what the current user is allowed to see, enforced by PostgreSQL itself.
This is not optional or bypassable — it is the architecture.
