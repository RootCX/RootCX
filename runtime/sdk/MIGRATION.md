# Migrating to @rootcx/sdk 0.19

This release removes desktop integration and makes authentication presentation
an application responsibility.

- `AuthGate` requires `renderForm`. Its typed `AuthFormSlotProps` provide the
  authentication state, providers, mode switching and submission handlers.
- Compose that form from `@rootcx/ui` 0.9. The CLI scaffold includes
  `src/components/auth-form.tsx` with `AuthForm` and `AuthLoading`.
- Local email/password registration and login have been removed from Core, the
  CLI and the SDK. Remove calls to `login`/`register` and the old `RegisterInput`
  type. `AuthSession` names the session returned by magic-link consumption.
- `AuthFormSlotProps` now contains only `appTitle`, `providers`, `onOidcLogin`,
  `error` and `submitting`. The fallback UI offers SSO retry or provider choice.
- Pass `renderLoading` for a themed loading view.
- `AuthGate` automatically redirects when one SSO provider is configured.
  Multiple providers keep the selection screen. With none, the fallback should
  explain that the administrator needs to configure SSO.
- Use `autoOidcLogin={false}` to always offer provider selection. Automatic
  retries are suppressed per Core and browser tab after interrupted SSO or
  explicit sign-out until successful login; manual sign-in remains available.
  If session storage is unavailable, the fallback avoids a redirect loop.
- SSO preserves the requested route, query parameters and page anchor.
- Core no longer advertises `passwordLoginEnabled` or `setupRequired` in auth
  mode. Initial users are provisioned by the configured identity provider.
- The Core migration removes stored local password hashes. User IDs, SSO
  identities, sessions, role assignments and application data are retained.
  Configure OIDC before upgrading a standalone installation.
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
