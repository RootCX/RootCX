---
name: rootcx-ui
description: Building RootCX app frontends with @rootcx/ui components, Tailwind v4 styling, AppShell/Sidebar layout, forms, data tables, dark mode, and the AuthGate entry pattern. Load references/components.md for the full component catalogue.
version: 0.1.0
---

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

See `references/components.md` for the full tables of primitives, layout, data, forms, and feedback components. Load it when you need exact prop signatures.

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

## App entry pattern

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
