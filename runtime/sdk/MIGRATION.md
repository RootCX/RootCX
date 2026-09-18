# Migrating to @rootcx/sdk 0.19

This release removes desktop integration and makes authentication presentation
an application responsibility.

- `AuthGate` requires `renderForm`. Its typed `AuthFormSlotProps` provide the
  authentication state, providers, mode switching and submission handlers.
- Compose that form from `@rootcx/ui` 0.9. The CLI scaffold includes
  `src/components/auth-form.tsx` with `AuthForm` and `AuthLoading`.
- Keep input names `email`, `password` and, when registering, `confirmPassword`.
  `AuthGate` still handles password confirmation, submission and error messages.
- Pass `renderLoading` for a themed loading view.
- OIDC and integration authentication use browser navigation and windows.
- Remove Tailwind source scanning of the SDK: it no longer contains styled views.

```tsx
<AuthGate
  appTitle="My app"
  renderForm={(props) => <AuthForm {...props} />}
  renderLoading={AuthLoading}
>
  {({ user, logout }) => <AppContent user={user} onLogout={logout} />}
</AuthGate>
```

Publish SDK 0.19 and UI 0.9 before releasing the CLI with this scaffold.
