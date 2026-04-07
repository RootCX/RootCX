# RootCX UI Components

## Primitives

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

## Layout

| Component | Key props |
|-----------|-----------|
| `AppShell` | `defaultOpen`, `sidebarWidth` — wraps `AppShellSidebar` + `AppShellMain` |
| `Sidebar` | `header`, `footer` |
| `SidebarSection` | `title`, `collapsible`, `defaultOpen` |
| `SidebarItem` | `icon`, `label`, `badge`, `active`, `onClick` |
| `PageHeader` | `title`, `description`, `breadcrumbs`, `actions`, `onBack` |
| `EmptyState` | `icon`, `title`, `description`, `action` |
| `useSidebar()` | returns `{ open, setOpen, toggle }` |

## Data

| Component | Key props |
|-----------|-----------|
| `DataTable` | `data`, `columns` (ColumnDef[]), `loading`, `searchable`, `pageSize`, `rowCount`, `onPaginationChange(PaginationState)`, `onSortingChange(SortingState)`, `selectable`, `resizable`, `rowActions` [{label,icon,onClick,destructive}], `bulkActions`, `emptyState`, `onRowClick`. Server-side: pass `rowCount`+`onPaginationChange` for pagination, `onSortingChange` for sorting — tanstack `manualPagination`/`manualSorting` enabled automatically. Types `SortingState`, `PaginationState` re-exported from `@rootcx/ui`. |
| `KPICard` | `label`, `value`, `trend`, `icon` |
| `StatusBadge` | `status` — auto-colors: active→green, pending→yellow, error→red |

## Forms

| Component | Key props |
|-----------|-----------|
| `FormDialog` | `open`, `onOpenChange`, `title`, `description`, `fields` [{name,label,type,required,options}], `defaultValues`, `onSubmit`, `submitLabel`, `destructive` |
| `FormField` | `field`, `value`, `onChange`, `error` |
| `SearchInput` | `value`, `onChange`, `placeholder`, `debounceMs` |
| `FilterBar` | `children` |

## Feedback

| Component | Usage |
|-----------|-------|
| `toast.success/error/info/warning()` | place `<Toaster />` at app root |
| `ConfirmDialog` | destructive confirmation dialog |
| `LoadingState` | `variant="spinner"` or `variant="skeleton"` |
| `ErrorState` | error message + optional retry button |
