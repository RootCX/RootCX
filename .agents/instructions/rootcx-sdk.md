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

- `id`, `created_at`, `updated_at` are auto-generated — never include in `fields`
- `entity_link` requires `"references": { "entity": "<name>", "field": "id" }`
- `"required": true` = mandatory on create; omit key for optional
- `"enum_values": [...]` restricts text fields to fixed values

---

## SDK Hooks

Never use `useState` with mock data. All data comes from hooks.

### useAppCollection

```tsx
const { data, loading, error, refetch, create, update, remove } = useAppCollection<T>(appId, entityName);
// create(fields) => Promise<T>
// update(id, fields) => Promise<T>
// remove(id) => Promise<void>
```

### useAppRecord

```tsx
const { data, loading, error, update, remove } = useAppRecord<T>(appId, entityName, recordId);
// update(fields) => Promise<T>
// remove() => Promise<void>
```

### useRuntimeStatus

```tsx
const { connected, loading } = useRuntimeStatus();
```

---

## Record shape

Records are **flat objects** — no `.fields` wrapper. Auto-fields: `id`, `created_at`, `updated_at`.
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
| `DataTable` | `data`, `columns` (ColumnDef[]), `loading`, `searchable`, `pagination`, `pageSize`, `selectable`, `rowActions` [{label,icon,onClick,destructive}], `bulkActions`, `emptyState`, `onRowClick` |
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
  {({ user, logout }) => (
    <AppShell>
      <AppShellSidebar>
        <Sidebar header={...} footer={...}>
          <SidebarItem icon={...} label="..." active={...} onClick={...} />
        </Sidebar>
      </AppShellSidebar>
      <AppShellMain>{/* views */}</AppShellMain>
      <Toaster />
    </AppShell>
  )}
</AuthGate>
```

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
