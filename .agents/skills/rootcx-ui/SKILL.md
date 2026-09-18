---
name: rootcx-ui
description: Build RootCX app interfaces with the external RootCX UI theme, composable shadcn/Radix components and Tailwind CSS 4. Forms, navigation, status meanings and table behavior belong to each app.
---

# RootCX UI & Styling

Stack: React 19, Tailwind CSS v4, shadcn/ui with Radix, Tabler icons.
`@rootcx/ui` is published from the separate `rootcx-ui` repository and distributes its components and light theme.

Before editing a generated app, check `package.json` and `src/globals.css`.
This scaffold requires `@rootcx/ui` 0.9 and `@rootcx/sdk` 0.19. An older CLI can
still generate the previous dependencies and theme. Use the updated scaffold and
verify that the required releases are published; a local package archive only
verifies local development. Do not fall back to the old UI or copy its theme.

## Theme and components

- The scaffold imports `@rootcx/ui/theme.css` in `src/globals.css`. This includes
  Tailwind, animations, Inter, the theme, and source detection for package components.
- Keep the supplied theme's typography, colors, materials, sizes and variants.
  Use semantic tokens (`bg-background`, `text-foreground`, `border-border`,
  `text-muted-foreground`) and Tailwind for layout.
- There is no `ThemeProvider`, `useTheme`, or dark mode in this theme.
- Use the themed components from `@rootcx/ui`, or individual entry points such as
  `@rootcx/ui/components/button`. These are composable shadcn components, not an
  application framework.
- Create application-specific components in `src/components/`. The application owns
  routes, page composition, form state and validation, status meanings, data queries,
  sorting, pagination, selection and actions.
- `components.json` is configured for Radix Nova and Tabler. Additional shadcn
  components can be installed into the app when needed. Use the existing theme and
  check the generated component's styling; upstream defaults may need alignment
  with the RootCX variants.
- Use `cn()` from `@/lib/utils` for conditional classes.
- `TooltipProvider` and `Toaster` are mounted in the generated entry point.
  Import `toast` directly from `sonner`.

## Composition

See [UI components](./references/components.md) for the available building blocks.

```tsx
import {
  Badge, Button,
  Dialog, DialogContent, DialogHeader, DialogTitle,
  Field, FieldGroup, FieldLabel, FieldError, Input,
} from "@rootcx/ui";
import { toast } from "sonner";
```

Build forms with `FieldGroup`, `Field`, a label and the appropriate input. Put them
inside `Dialog` when needed. The app implements validation and submission; there is
no field-schema renderer. Set `data-invalid` on `Field`, and `aria-invalid` on the
control, and associate help/errors with the control.

Choose `Badge` variants explicitly according to the app's semantics; no component
infers a color from a status string.

Build tables with `Table`, `TableHeader`, `TableBody`, `TableRow`, `TableHead` and
`TableCell`. Add TanStack Table in the app when advanced sorting, pagination or
selection is needed. There is no universal `DataTable` API.

Compose navigation with `SidebarProvider`, `Sidebar`, `SidebarHeader`,
`SidebarContent`, `SidebarGroup`, `SidebarMenu`, `SidebarMenuItem`,
`SidebarMenuButton`, `SidebarFooter`, `SidebarInset` and `SidebarTrigger`.
This is one available layout, not a required application shell.

```tsx
<SidebarMenuItem>
  <SidebarMenuButton asChild isActive={pathname === "/contacts"}>
    <NavLink to="/contacts"><IconUsers /><span>Contacts</span></NavLink>
  </SidebarMenuButton>
</SidebarMenuItem>
```

The design repository also provides optional visual compositions (`Page`,
`PageHeader`, `PageTitle`, `MetricCard`, `PropertyList`). They accept composed
content; they do not manage data or dictate an app's workflow. In particular,
`PageHeader` is a container, not the old `title`/`description` wrapper.

## Routing and runtime

`BrowserRouter` in `main.tsx` uses `basename={import.meta.env.BASE_URL}` because apps
are served under `/apps/<app_id>/`. Keep this basename. Use `react-router-dom` for
navigation, `useParams()` for record pages and `useSearchParams()` for shareable
filters, sort and pagination. Include a catch-all route.

Keep runtime/authentication concerns in `@rootcx/sdk`: `RuntimeProvider`,
`AuthGate`, data hooks and the runtime client. Call React hooks at the top level of
components, not inside an `AuthGate` render callback.

`AuthGate` in SDK 0.19 requires `renderForm`; it has no styled default form.
Keep the generated `AuthForm` and `AuthLoading` slots. They compose
`@rootcx/ui` components for login, registration, SSO and loading; authentication
state, validation and submission remain in the SDK.

Agent chat presentation and scrolling live in generated app-local components.
They are editable application code, not exports from `@rootcx/ui`.
