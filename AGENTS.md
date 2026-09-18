# Repository boundaries

- This repository owns Core, the CLI, scaffolding and the SDK.
- `@rootcx/ui` is owned and published by the separate `rootcx-ui` repository.
  Consume it as a dependency. Make shared component and theme changes there.
- Apps use React 19, Tailwind CSS 4 and composable shadcn/ui Radix components.
  Import `@rootcx/ui/theme.css` once; keep app behavior in app-local components.
- Read `.agents/skills/rootcx-ui/SKILL.md` before changing frontend templates.
  Read `docs/PACKAGES.md` for package ownership and release order.
- Development happens in external editors and coding agents through the CLI.
  Core and CLI builds must not require a desktop IDE or embedded coding engine.

Follow the coding standards in `CLAUDE.md`.
