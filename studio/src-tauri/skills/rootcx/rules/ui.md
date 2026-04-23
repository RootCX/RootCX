# RootCX UI & Styling

Stack: **Tailwind CSS v4** + **`@rootcx/ui`** (pre-configured).

## Rules

- Import all UI from `@rootcx/ui` — never duplicate library components
- `cn()` from `@/lib/utils` for conditional classes — never string concatenation
- Tailwind utilities for layout/spacing — never inline `style={{}}`
- Icons: `@tabler/icons-react`
- Custom components in `src/components/` only when `@rootcx/ui` doesn't cover the need
- Prefer semantic color tokens (`bg-background`, `text-foreground`, `bg-card`, `border-border`, `text-muted-foreground`, `bg-accent`, `bg-primary`) over hardcoded colors. Avoid `dark:` prefix — CSS variables switch automatically.
- Dark mode: `ThemeProvider` wraps app in `main.tsx` (scaffold does this). Use `useTheme()` for toggle: `const { theme, setTheme } = useTheme()`. Values: `"dark"`, `"light"`, `"system"`.

## Imports

```tsx
import { BrowserRouter, Routes, Route, Navigate, useParams, useSearchParams } from "react-router-dom";
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

## Component catalogue

See [UI Components](./ui-components.md) for the full tables of primitives, layout, data, forms, and feedback components with prop signatures.

---

## DataTable usage

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

## Select with "All" filter

```tsx
<Select value={filter ?? "all"} onValueChange={(v) => setFilter(v === "all" ? undefined : v)}>
  <SelectTrigger><SelectValue /></SelectTrigger>
  <SelectContent>
    <SelectItem value="all">All</SelectItem>
    <SelectItem value="tier_1">Tier 1</SelectItem>
  </SelectContent>
</Select>
```

## Routing

`BrowserRouter` wraps the app in `main.tsx` (scaffold does this) with `basename={import.meta.env.BASE_URL}` — required because apps are served under `/apps/<app_id>/` in production. Use `react-router-dom` for all navigation.

**Rules:**
- Use `<SidebarItem to="/path" />` for sidebar navigation (renders `NavLink`, active state automatic)
- Use `<SidebarItem onClick={...} />` only for actions (theme toggle, logout) -- not navigation
- Define routes with `<Routes>` + `<Route>` inside `AppShellMain`
- Use `useParams()` for record detail pages (`/contacts/:id`)
- Sync list state to URL with `useSearchParams()`: page, sort, filters. Read params on mount, update params on user action. This gives deep linking and back/forward for free.
- Always add a catch-all `<Route path="*" element={<Navigate to="/" replace />} />`

## App entry pattern

```tsx
// main.tsx: <BrowserRouter basename={import.meta.env.BASE_URL}> > RuntimeProvider > ThemeProvider

// App.tsx
<AuthGate appTitle="<Name>">
  {({ user, logout }) => {
    const { theme, setTheme } = useTheme();
    return (
      <AppShell>
        <AppShellSidebar>
          <Sidebar header={...} footer={...}>
            <SidebarItem to="/" icon={...} label="Dashboard" />
            <SidebarItem to="/contacts" icon={...} label="Contacts" />
            <SidebarItem
              icon={theme === "dark" ? <IconSun /> : <IconMoon />}
              label={theme === "dark" ? "Light mode" : "Dark mode"}
              onClick={() => setTheme(theme === "dark" ? "light" : "dark")}
            />
          </Sidebar>
        </AppShellSidebar>
        <AppShellMain>
          <Routes>
            <Route path="/" element={<DashboardPage />} />
            <Route path="/contacts" element={<ContactsPage />} />
            <Route path="/contacts/:id" element={<ContactDetailPage />} />
            <Route path="*" element={<Navigate to="/" replace />} />
          </Routes>
        </AppShellMain>
        <Toaster />
      </AppShell>
    );
  }}
</AuthGate>
```
