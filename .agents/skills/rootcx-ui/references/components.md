# RootCX UI components

`@rootcx/ui` exports the components of the RootCX design repository, using
shadcn/Radix composition. Inspect the installed types/source for exact props.
Components are also available from `@rootcx/ui/components/<name>`.

| Area | Components |
| --- | --- |
| Actions | `Button`, `ButtonGroup`, `Toggle`, `ToggleGroup` |
| Fields | `Input`, `Textarea`, `Label`, `Field`, `FieldGroup`, `FieldLabel`, `FieldDescription`, `FieldError`, `FieldSet`, `FieldLegend`, `InputGroup`, `NativeSelect`, `Select`, `Checkbox`, `Switch` |
| Surfaces | `Card`, `CardHeader`, `CardTitle`, `CardDescription`, `CardContent`, `CardFooter` |
| Overlays | `Dialog`, `AlertDialog`, `Sheet`, `Popover`, `Tooltip`, with their trigger/content/title parts |
| Navigation | `SidebarProvider`, `Sidebar` and its composition parts, `Breadcrumb`, `Tabs` |
| Menus | `DropdownMenu` and its groups, items and submenus |
| Display | `Badge`, `BadgeDot`, `ColorSwatch`, `Avatar`, `Table`, `Kbd` |
| Feedback | `Alert`, `Empty`, `Skeleton`, `Spinner`, `Progress`, `Toaster` |
| Layout | `Separator`, `ScrollArea` |
| RootCX visual compositions | `PageFrame`, `PagePanel`, `PageTopbar`, `PageTopbarActions`, `Page`, `PageHeader`, `PageHeading`, `PageTitle`, `PageDescription`, `PageActions`, `PageToolbar`, `IconTile`, `MetricCard`, `MetricStrip`, `Metric`, `PropertyList`, `PropertyRow`, `PropertyLabel`, `PropertyValue`, `PropertySection` |

## Theme variants

- `Button`: `default`, `outline`, `secondary`, `ghost`, `toolbar`, `soft`,
  `destructive`, `link`. Sizes: `default`, `xs`, `sm`, `lg`, `icon`, `icon-xs`,
  `icon-sm`, `icon-lg`.
- `Badge`: `default`, `neutral`, `info`, `success`, `warning`, `destructive`,
  `violet`, `secondary`, and pastel `mint`, `teal`, `lavender`, `pink`, `yellow`, `blue`.
- `TabsList`: `default`, `segmented`, `line`.
- `ToggleGroup`: `default`, `outline`, `segmented`.
- `Card`: `default`, `raised`.
- `SelectTrigger`: `default`, `filled`.

Use full compositions: a dialog needs a title; select items belong in a group;
tabs triggers belong in a tabs list. Pass `asChild` for custom Radix triggers.

## Application-owned behavior

Build status displays with `Badge`, forms with `Field` and controls, confirmations
with `AlertDialog`, and loading/empty/error views with the matching primitives.
Define their content and behavior in the app. Table sorting/pagination, navigation
structure and form validation are not UI package responsibilities.

Removed APIs include `AppShell*`, `SidebarItem`, `SidebarSection`, `StatusBadge`,
`KPICard`, `DataTable`, `FormField`, `FormDialog`, `FilterBar`, `SearchInput`,
`ConfirmDialog`, `LoadingState`, `ErrorState`, `EmptyState`, `ThemeProvider`,
`useTheme`, `ChatScrollArea`, `useAutoScroll` and `Markdown`.
Do not generate imports of these from `@rootcx/ui`.

`PageHeader` now takes children, and `Sidebar`/`useSidebar` follow the new
`SidebarProvider` composition. They are not compatible aliases for the old API.
Use `React.ComponentProps<typeof Button>` or the corresponding component for prop
types. Import `toast` from `sonner`; import TanStack types from TanStack if installed.
