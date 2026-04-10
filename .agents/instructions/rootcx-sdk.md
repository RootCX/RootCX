# RootCX SDK

Apps require: `manifest.json` (data contract) + React code using `@rootcx/sdk` hooks and `@rootcx/ui` components.

## manifest.json

```json
{
  "appId": "<id>",
  "name": "<Name>",
  "version": "0.0.1",
  "description": "<description>",
  "dataContract": [
    {
      "entityName": "<entity>",
      "fields": [
        { "name": "<field>", "type": "<type>", "required": true },
        { "name": "<field>", "type": "entity_link", "references": { "entity": "<target>", "field": "id" } },
        { "name": "<field>", "type": "text", "enum_values": ["a", "b", "c"] }
      ]
    }
  ],
  "permissions": {
    "permissions": [
      { "key": "<entity>.<action>", "description": "<description>" }
    ]
  }
}
```

### Field types

`text` `number` `boolean` `date` `timestamp` `json` `file` `entity_link` `[text]` `[number]`

### Rules

- `id`, `created_at`, `updated_at` are auto-generated — omit from `fields`
- `entity_link` requires `"references": { "entity": "<target>", "field": "id" }`. `<target>` is `"<entity>"` (same app) or `"core:users"` (FK → `rootcx_system.users`, `ON DELETE SET NULL`). Cross-app refs not yet supported.
- `"required": true` = mandatory on create; omit key for optional
- `"enum_values": [...]` restricts text fields to fixed values

---

## Schema Sync

On install/deploy, Core runs `CREATE SCHEMA IF NOT EXISTS` + `CREATE TABLE IF NOT EXISTS` for each entity in `dataContract`. Then `sync_schema` diffs DB vs manifest and auto-applies all changes (add/drop columns, alter types, nullability, defaults, check constraints). Studio shows a confirmation dialog before applying.

### Manifest ↔ DB contract

`dataContract` fields map to columns. Auto-columns (`id UUID`, `created_at`, `updated_at`) added by Core — omit from manifest `fields`. Type mapping: `text`→`TEXT`, `number`→`DOUBLE PRECISION`, `boolean`→`BOOLEAN`, `date`→`DATE`, `timestamp`→`TIMESTAMPTZ`, `json`→`JSONB`, `file`→`TEXT`, `entity_link`→`UUID`, `[text]`→`TEXT[]`, `[number]`→`DOUBLE PRECISION[]`.

---

## SDK Hooks

All data from hooks. Types exported from `@rootcx/sdk`. Never use `useState` with mock data.

### useAppCollection

```tsx
useAppCollection<T>(appId, entity, query?: QueryOptions)
```

Returns: `{ data: T[], total: number, loading, error, refetch, create, bulkCreate, update, remove }`

Without `query`: `GET /collections/{entity}` (full list). With `query`: `POST /collections/{entity}/query` (server-side filter/sort/paginate). Auto re-fetches on `query` change.

**Cross-app reads:** `appId` can be any installed app or integration — not limited to the current app. Use another app's ID to read its collections. User must have read permissions on the target app.

`create(fields) → T` · `bulkCreate(fields[]) → T[]` · `update(id, fields) → T` · `remove(id) → void`

### QueryOptions

```tsx
{ where?: WhereClause, orderBy?: string, order?: "asc"|"desc", limit?: number, offset?: number }
```

**Where operators:** `$eq` `$ne` `$gt` `$gte` `$lt` `$lte` `$like` `$ilike` `$in` `$nin` `$contains` `$isNull`
**Logical:** `$and` `$or` (WhereClause[]) · `$not` (WhereClause)
**Shorthand:** `{field: value}` = `{field: {$eq: value}}` · `{field: null}` = IS NULL

```tsx
useAppCollection<Invoice>(appId, "invoice", {
  where: { $or: [{status: "pending"}, {amount: {$gte: 1000}}], date: {$gte: "2026-01-01", $lte: "2026-03-31"}, name: {$ilike: "%acme%"} },
  orderBy: "date", order: "desc", limit: 50, offset: 0,
});
```

### useAppRecord

```tsx
useAppRecord<T>(appId, entity, recordId | null)
```

Returns: `{ data: T|null, loading, error, refetch, update, remove }`

`update(fields) → T` · `remove() → void` · `null` id skips fetch

### useIntegration

```tsx
const { connected, loading, connect, submitCredentials, disconnect, call } = useIntegration(appId, integrationId);
```

`connect()` → OAuth redirect or `{type:"credentials", schema}` · `call(actionId, params?) → result`

**Call `list_integrations` first. Never guess action IDs.**

### useCoreCollection

```tsx
useCoreCollection<T>(entity)
```

Returns: `{ data: T[], loading, error, refetch }`

Read-only access to core platform entities. `GET /api/v1/{entity}` (not app collections).

**`core:users` in manifest `entity_link` references → use `useCoreCollection("users")` to fetch org members. Do NOT use `useAppCollection` with `core:users` — it will 404.**

### useCrons

```tsx
const { data, loading, error, refetch, create, update, remove, trigger } = useCrons(appId);
```

Returns: `{ data: CronSchedule[], loading, error, refetch, create, update, remove, trigger }`

CRUD for scheduled jobs. Crons fire via pg_cron → pgmq → Core scheduler → worker `onJob`.

`create({ name, schedule, payload?, timezone?, overlapPolicy? }) → CronSchedule` · `update(id, { schedule?, payload?, enabled?, overlapPolicy? }) → CronSchedule` · `remove(id) → void` · `trigger(id) → { msgId }` (manual fire)

**schedule:** 5-field cron (`"0 9 * * *"` = daily 9am) or `"N seconds"` interval (`"10 seconds"`, 1-59). `$` = last day of month. All times GMT unless timezone set. **overlapPolicy:** `"skip"` (default, dedup) or `"queue"`. **payload:** arbitrary JSON passed to worker `onJob`.

```tsx
await create({ name: `check-${campaignId}`, schedule: "0 9 * * *", payload: { campaignId } });
await update(cronId, { enabled: false }); // pause
await trigger(cronId); // manual fire
```

### useRuntimeClient

```tsx
const client = useRuntimeClient();
```

`client.queryRecords<T>(appId, entity, QueryOptions) → {data, total}` · `client.rpc(appId, method, params?) → unknown` · `client.core().collection<T>(entity).list() → T[]` · `client.core().collection<T>(entity).get(id) → T`

For imperative calls in event handlers. For reactive data, use `useAppCollection` / `useCoreCollection`.

---

## Record shape

Records are **flat objects**. Auto-fields: `id`, `created_at`, `updated_at`.
When creating/updating, pass only user-defined fields.

---

## UI & Styling

Stack: **Tailwind CSS v4** + **`@rootcx/ui`** (pre-configured).

### Rules

- Import all UI from `@rootcx/ui` — never duplicate library components
- `cn()` from `@/lib/utils` for conditional classes — never string concatenation
- Tailwind utilities for layout/spacing — never inline `style={{}}`
- Icons: `@tabler/icons-react`
- Custom components in `src/components/` only when `@rootcx/ui` doesn't cover the need
- Prefer semantic color tokens (`bg-background`, `text-foreground`, `bg-card`, `border-border`, `text-muted-foreground`, `bg-accent`, `bg-primary`) over hardcoded colors. Avoid `dark:` prefix — CSS variables switch automatically.
- Dark mode: `ThemeProvider` wraps app in `main.tsx` (scaffold does this). Use `useTheme()` for toggle: `const { theme, setTheme } = useTheme()`. Values: `"dark"`, `"light"`, `"system"`.

### Imports

```tsx
import { Button, Input, Label, Card, CardHeader, CardTitle, CardContent, CardDescription,
  Badge, Select, SelectTrigger, SelectContent, SelectItem, SelectValue,
  Dialog, DialogContent, DialogHeader, DialogFooter, DialogTitle, DialogDescription,
  Tabs, TabsList, TabsTrigger, TabsContent,
  Table, TableBody, TableCell, TableHead, TableHeader, TableRow,
  Separator, ScrollArea, Tooltip, TooltipTrigger, TooltipContent, TooltipProvider,
  DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger,
  Popover, PopoverTrigger, PopoverContent,
  Switch, Textarea,
  AppShell, AppShellSidebar, AppShellMain,
  Sidebar, SidebarItem, SidebarSection,
  PageHeader, DataTable, FormDialog, StatusBadge, EmptyState,
  KPICard, FormField, SearchInput, FilterBar,
  LoadingState, ErrorState, ConfirmDialog,
  toast, Toaster,
  ThemeProvider, useTheme,
} from "@rootcx/ui";
import { IconPlus, IconTrash, IconEdit } from "@tabler/icons-react";
import { cn } from "@/lib/utils";
import { AuthGate } from "@rootcx/sdk";
import type { ColumnDef } from "@tanstack/react-table";
```

---

## UI Components

### Primitives

| Component | Notes |
|-----------|-------|
| `Button` | variants: default/destructive/outline/secondary/ghost/link; sizes: default/sm/lg/icon |
| `Input` | standard text input |
| `Label` | Radix-accessible form label |
| `Card` (+Header/Title/Description/Content) | card container |
| `Badge` | variants: default/secondary/destructive/outline |
| `Select` (+Trigger/Content/Item/Value) | Radix dropdown |
| `Dialog` (+Content/Header/Footer/Title/Description) | modal |
| `Tabs` (+List/Trigger/Content) | tab nav |
| `Table` (+Header/Body/Row/Head/Cell) | styled HTML table |
| `Separator` | divider |
| `ScrollArea` | custom scrollbar |
| `Tooltip` (+Trigger/Content/Provider) | hover tooltip |
| `DropdownMenu` (+Trigger/Content/Item) | action menu |
| `Popover` (+Trigger/Content) | floating panel |
| `Switch` | toggle |
| `Textarea` | multi-line input |

### Layout

| Component | Key props |
|-----------|-----------|
| `AppShell` | `defaultOpen`, `sidebarWidth` — wraps `AppShellSidebar` + `AppShellMain` |
| `Sidebar` | `header`, `footer` |
| `SidebarSection` | `title`, `collapsible`, `defaultOpen` |
| `SidebarItem` | `icon`, `label`, `badge`, `active`, `onClick` |
| `PageHeader` | `title`, `description`, `breadcrumbs`, `actions`, `onBack` |
| `EmptyState` | `icon`, `title`, `description`, `action` |
| `useSidebar()` | returns `{ open, setOpen, toggle }` |

### Data

| Component | Key props |
|-----------|-----------|
| `DataTable` | `data`, `columns` (ColumnDef[]), `loading`, `searchable`, `pageSize`, `rowCount`, `onPaginationChange(PaginationState)`, `onSortingChange(SortingState)`, `selectable`, `resizable`, `rowActions` [{label,icon,onClick,destructive}], `bulkActions`, `emptyState`, `onRowClick`. Server-side: pass `rowCount`+`onPaginationChange` for pagination, `onSortingChange` for sorting — tanstack `manualPagination`/`manualSorting` enabled automatically. Types `SortingState`, `PaginationState` re-exported from `@rootcx/ui`. |
| `KPICard` | `label`, `value`, `trend`, `icon` |
| `StatusBadge` | `status` — auto-colors: active→green, pending→yellow, error→red |

### Forms

| Component | Key props |
|-----------|-----------|
| `FormDialog` | `open`, `onOpenChange`, `title`, `description`, `fields` [{name,label,type,required,options}], `defaultValues`, `onSubmit`, `submitLabel`, `destructive` |
| `FormField` | `field`, `value`, `onChange`, `error` |
| `SearchInput` | `value`, `onChange`, `placeholder`, `debounceMs` |
| `FilterBar` | `children` |

### Feedback

| Component | Usage |
|-----------|-------|
| `toast.success/error/info/warning()` | place `<Toaster />` at app root |
| `ConfirmDialog` | destructive confirmation dialog |
| `LoadingState` | `variant="spinner"` or `variant="skeleton"` |
| `ErrorState` | error message + optional retry button |

### DataTable usage

```tsx
const columns: ColumnDef<T, unknown>[] = [
  { accessorKey: "name", header: "Name" },
  { accessorKey: "status", header: "Status", cell: ({ row }) => <StatusBadge status={row.original.status} /> },
];

<DataTable data={items} columns={columns} loading={loading} searchable selectable
  rowCount={totalCount} onPaginationChange={({ pageIndex, pageSize }) => fetchPage(pageIndex, pageSize)}
  onSortingChange={(s) => s[0] && fetchSorted(s[0].id, s[0].desc ? "desc" : "asc")}
  rowActions={[
    { label: "Edit", icon: <IconEdit className="h-4 w-4" />, onClick: (row) => edit(row) },
    { label: "Delete", icon: <IconTrash className="h-4 w-4" />, onClick: (row) => remove(row.id), destructive: true },
  ]}
  bulkActions={[{ label: "Delete selected", onClick: (rows) => rows.forEach(r => remove(r.id)), destructive: true }]}
  emptyState={<EmptyState title="No items" description="Add your first item" />}
/>
```

### App entry pattern

```tsx
<AuthGate appTitle="<Name>">
  {({ user, logout }) => {
    const { theme, setTheme } = useTheme();
    return (
      <AppShell>
        <AppShellSidebar>
          <Sidebar header={...} footer={...}>
            <SidebarItem icon={...} label="..." active={...} onClick={...} />
            <SidebarItem
              icon={theme === "dark" ? <IconSun /> : <IconMoon />}
              label={theme === "dark" ? "Light mode" : "Dark mode"}
              onClick={() => setTheme(theme === "dark" ? "light" : "dark")}
            />
          </Sidebar>
        </AppShellSidebar>
        <AppShellMain>{/* views */}</AppShellMain>
        <Toaster />
      </AppShell>
    );
  }}
</AuthGate>
```

---

## Backend Workers

Apps can have a `backend/` directory with a Bun worker for server-side logic. Core manages lifecycle (spawn, crash recovery, shutdown). IPC via JSON-lines on stdin/stdout.

Deps: add `backend/package.json` for backend-only npm deps. Core runs `bun install` there at deploy. Do NOT put backend deps in the root `package.json` (that one is for the frontend/Vite).

### IPC protocol

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

### Data access

- **Simple CRUD**: use Core REST API via `runtime_url` with `caller.authToken`
- **Custom SQL** (transactions, sequences, JOINs): connect to PostgreSQL via `database_url` from discover
- All apps share one PG instance — cross-app queries are possible
- **NEVER use SQLite or file-based storage** — PostgreSQL is the only database

### Core REST API — Collections

Base: `/api/v1/apps/{app_id}/collections/{entity}`

| Method | Path | Body | Response |
|--------|------|------|----------|
| GET | `/` | — | `T[]` |
| POST | `/` | `{field:value,...}` | `T` (201) |
| POST | `/bulk` | `[{...},...]` | `T[]` (201) |
| POST | `/query` | `QueryOptions` | `{data:T[],total:number}` |
| GET | `/{id}` | — | `T` |
| PATCH | `/{id}` | `{field:value,...}` | `T` |
| DELETE | `/{id}` | — | `{message:string}` |

**GET list — query params (flat, no bracket syntax):**
- Filter: field name directly as param → `?contact_id=uuid&status=active`
- `sort` — field name (must exist in entity or `created_at`/`updated_at`/`id`), default `created_at`
- `order` — `asc` or `desc`, default `desc`
- `limit` — 1–1000, no default (returns all if omitted)
- `offset` — integer ≥ 0

**POST /query — body (JSON):**
- `where` — nested filter object (see operators below)
- `orderBy` — field name, default `created_at`
- `order` — `asc`/`desc`, default `desc`
- `limit` — 1–1000, default 100
- `offset` — integer ≥ 0

**Where operators:** `$eq` `$ne` `$gt` `$gte` `$lt` `$lte` `$like` `$ilike` `$in` `$nin` `$contains` `$isNull`
**Logical:** `$and` `$or` (arrays) `$not` (object)
**Shorthand:** `{"field":"value"}` = `{"field":{"$eq":"value"}}`, `{"field":null}` = IS NULL

### Core REST API — Integrations

Bind — base `/api/v1/apps/{app_id}/integrations`:

| Method | Path | Body | Response |
|--------|------|------|----------|
| GET | `/` | — | bindings list |
| POST | `/` | `{integrationId, config?}` | bind |
| PATCH | `/{integration_id}` | `{config}` | update config |
| DELETE | `/{integration_id}` | — | unbind |

Actions + auth — base `/api/v1/integrations`:

| Method | Path | Body | Response |
|--------|------|------|----------|
| POST | `/{integration_id}/actions/{action_id}` | action input | action result |
| GET | `/{integration_id}/auth` | — | `{connected,type}` |
| POST | `/{integration_id}/auth/start` | — | OAuth url or credential schema |
| POST | `/{integration_id}/auth/credentials` | `{field:value,...}` | — |
| DELETE | `/{integration_id}/auth` | — | disconnect |

**From a worker:** `POST {runtime_url}/api/v1/integrations/{integration_id}/actions/{action_id}` with `Authorization: Bearer {authToken}`, body = action input.

### Core REST API — Jobs

Async job queue managed by Core. Workers enqueue jobs via REST, Core dispatches them back to the worker via IPC.

Base: `/api/v1/apps/{app_id}/jobs`

| Method | Path | Body | Response |
|--------|------|------|----------|
| POST | `/` | `{payload:{...}}` | `{job_id}` (201) |
| GET | `/` | — | `Job[]` (query: `status`, `limit`) |
| GET | `/{job_id}` | — | `Job` |

**Job statuses:** `pending` → `running` → `completed` | `failed`

**Flow:**
1. Worker (or frontend) enqueues: `POST /api/v1/apps/{app_id}/jobs` with `{payload:{...}}` + `Authorization: Bearer {authToken}`
2. Core scheduler claims pending jobs and dispatches to worker via IPC: `{ type: "job", id, payload, caller }` — `caller` has `userId`, `username`, `authToken` (short-lived JWT minted by Core from the enqueuing user)
3. Worker processes and responds: `{ type: "job_result", id, result }` or `{ type: "job_result", id, error }`
4. Use `caller.authToken` in job handlers for authenticated Core API calls (collections, integrations, etc.)

Use jobs for long-running work (bulk fetches, batch imports, async syncs) that would exceed the 30s RPC timeout.

### Core REST API — Crons

Scheduled jobs via pg_cron. Crons fire → pgmq → scheduler → worker `onJob`.

Base: `/api/v1/apps/{app_id}/crons`

| Method | Path | Body | Response |
|--------|------|------|----------|
| POST | `/` | `{name, schedule, payload?, timezone?, overlapPolicy?}` | `CronSchedule` (201) |
| GET | `/` | — | `CronSchedule[]` |
| PATCH | `/{id}` | `{schedule?, payload?, overlapPolicy?, enabled?}` | `CronSchedule` |
| DELETE | `/{id}` | — | `{message}` |
| POST | `/{id}/trigger` | — | `{msgId}` |

**schedule:** 5-field cron (`min hr dom mon dow`) or `"N seconds"` interval (`"10 seconds"`, 1-59). `$` in dom = last day of month. All times GMT unless timezone set. **overlapPolicy:** `"skip"` (default) or `"queue"`. **enabled:** toggle on/off without deleting.

Cron payload arrives in worker `onJob(payload, caller, ctx)` with the configured payload + `cron_id`.

### Frontend → Worker

```tsx
const client = useRuntimeClient();
const result = await client.rpc(appId, "method_name", { ...params });
```

### Minimal worker template

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

### serve() API (v2)

Preferred over raw stdin/stdout. Prelude injects `serve()` globally:

```typescript
serve({
  rpc: {
    ping: async (params, caller, ctx) => ({ pong: true }),
  },
  onJob: async (payload, caller, ctx) => {
    ctx.log.info("job received");
    await ctx.collection("entity").insert({ field: "value" });
  },
  onStart: async (ctx) => {},
  onShutdown: () => {},
});
```

`ctx`: `{ appId, runtimeUrl, databaseUrl, credentials, log: { info, warn, error }, emit(name, data?), collection(entity): { insert, update }, uploadFile() }`

**Cron → onJob:** `useCrons().create({ schedule, payload })` → pg_cron → pgmq → scheduler → `onJob(payload)`.

### Rules

- Entry point: `index.ts` → `index.js` → `main.ts` → `main.js` → `src/index.ts`
- RPC timeout: 30s. Always respond with matching `id`
- Use `caller.authToken` for authenticated Core API calls from the worker
- Crash recovery: max 5 crashes in 60s → failed state

---

## AI Agents

Agents are apps with a `backend/` containing a LangGraph agent. Same manifest, same deploy, same RBAC. Core passes LLM credentials via IPC.

### Project structure

```
my-agent/
├── manifest.json                  # Data contract (no agent field)
├── .rootcx/launch.json
├── src/App.tsx                    # Chat UI
└── backend/
    ├── agent.json                 # Agent config (limits, memory, supervision)
    ├── agent/system.md            # System prompt
    ├── index.ts                   # LangGraph agent + IPC bridge
    └── package.json               # @langchain/langgraph, @langchain/openai, zod
```

### backend/agent.json

Agent config — read by Core at deploy time, separate from manifest:

```json
{
  "name": "<name>",
  "description": "<description>",
  "systemPrompt": "./agent/system.md",
  "memory": { "enabled": true },
  "limits": { "maxTurns": 50, "maxContextTokens": 100000, "keepRecentMessages": 10 },
  "supervision": { "mode": "autonomous" }
}
```

LLM provider selected at scaffold time. Platform secrets (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `AWS_BEARER_TOKEN_BEDROCK`) set via dashboard. Core passes credentials to agent at startup.

### Tools & permissions

All registered tools available via IPC. RBAC permissions declared in `manifest.json` `permissions.permissions[]` as `{ "key": "<entity>.<action>", "description": "..." }` strings.

### Backend code

The scaffold generates a single `index.ts` — the developer owns the code:
- LangGraph `createReactAgent` handles the ReAct loop and streaming
- Provider-specific LangChain SDK (ChatAnthropic, ChatOpenAI, ChatBedrockConverse)
- IPC bridge (JSON-lines stdin/stdout) connects to Core for tool calls and supervision
- Dependencies: `@langchain/langgraph`, provider package, `@langchain/core`, `zod`

### Invocation

```
POST /api/v1/apps/{app_id}/agent/invoke
{ "message": "...", "session_id": "optional-uuid" }
```

Response: SSE stream (`chunk`, `tool_call_started`, `tool_call_completed`, `approval_required`, `done`, `error` events).

Other endpoints:
- `GET /api/v1/apps/{app_id}/agent` — agent config
- `GET /api/v1/apps/{app_id}/agent/sessions` — list sessions
- `GET /api/v1/apps/{app_id}/agent/sessions/{session_id}` — get session
