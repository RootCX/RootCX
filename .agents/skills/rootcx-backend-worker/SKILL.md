---
name: rootcx-backend-worker
description: Writing a RootCX app backend/ Bun worker — JSON-lines IPC protocol with Core, discover/rpc/job/shutdown messages, caller auth, data access patterns, and the minimal worker template. Load when implementing backend/index.ts or handling RPC methods and jobs.
version: 0.1.0
---

# RootCX Backend Workers

Apps can have a `backend/` directory with a Bun worker for server-side logic. Core manages lifecycle (spawn, crash recovery, shutdown). IPC via JSON-lines on stdin/stdout.

Deps: add `backend/package.json` for backend-only npm deps. Core runs `bun install` there at deploy. Do NOT put backend deps in the root `package.json` (that one is for the frontend/Vite).

## IPC protocol

Core sends `discover` immediately after spawn. Worker listens on stdin, responds on stdout. JSON-lines (one JSON object per line).

**Messages Core → Worker:**
- `{ type: "discover", app_id, runtime_url, database_url, credentials }` — init handshake
- `{ type: "rpc", id, method, params, caller }` — caller includes `authToken` for Core API calls
- `{ type: "job", id, payload, caller }` — async job dispatch (caller has authToken if enqueued by a user)
- `{ type: "shutdown" }` — graceful exit

**Messages Worker → Core:**
- `{ type: "discover", methods: [...] }` — handshake response (list exposed RPC methods)
- `{ type: "rpc_response", id, result }` or `{ type: "rpc_response", id, error }`
- `{ type: "job_result", id, result }` or `{ type: "job_result", id, error }`
- `{ type: "log", level: "info"|"warn"|"error", message }` — structured logging

**Caller shape:** `{ userId: string, username: string, authToken?: string }`
- `authToken` is the caller's JWT — use it for `Authorization: Bearer` when calling Core REST API
- Always check `caller` for authorization in RPC handlers

## Data access

- **Simple CRUD**: use Core REST API via `runtime_url` with `caller.authToken` (see `rootcx-rest-api` skill)
- **Custom SQL** (transactions, sequences, JOINs): connect to PostgreSQL via `database_url` from discover
- All apps share one PG instance — cross-app queries are possible
- **NEVER use SQLite or file-based storage** — PostgreSQL is the only database

## Frontend → Worker

```tsx
const client = useRuntimeClient();
const result = await client.rpc(appId, "method_name", { ...params });
```

## Minimal worker template

See `templates/worker.ts` for the full minimal template. Core shape:

```typescript
import { createInterface } from "readline";
import postgres from "postgres";

interface Caller { userId: string; username: string; authToken?: string }

const write = (m: any) => process.stdout.write(JSON.stringify(m) + "\n");
const rl = createInterface({ input: process.stdin });
let sql: ReturnType<typeof postgres>;
let runtimeUrl: string;
let appId: string;

rl.on("line", (l) => {
  let m: any;
  try { m = JSON.parse(l); } catch { return; }

  switch (m.type) {
    case "discover":
      appId = m.app_id;
      runtimeUrl = m.runtime_url;
      sql = postgres(m.database_url);
      write({ type: "discover", methods: ["ping"] });
      break;
    case "rpc":
      handleRpc(m);
      break;
    case "shutdown":
      process.exit(0);
  }
});

async function handleRpc(m: any) {
  try {
    const result = await dispatch(m.method, m.params ?? {}, m.caller);
    write({ type: "rpc_response", id: m.id, result });
  } catch (e: any) {
    write({ type: "rpc_response", id: m.id, error: e.message });
  }
}

async function dispatch(method: string, params: any, caller: Caller | null): Promise<any> {
  switch (method) {
    case "ping": return { pong: true };
    default: throw new Error(`unknown method: ${method}`);
  }
}
```

## Rules

- Entry point: `index.ts` → `index.js` → `main.ts` → `main.js` → `src/index.ts`
- RPC timeout: 30s. Always respond with matching `id`
- Use `caller.authToken` for authenticated Core API calls from the worker
- Crash recovery: max 5 crashes in 60s → failed state
