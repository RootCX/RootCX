# Core 0.29: SSO authentication

Core no longer registers or authenticates local email/password accounts.
Interactive sign-in uses the configured OIDC provider. Governed magic links,
service-account credentials and existing sessions remain supported.

## Breaking changes

- `POST /api/v1/auth/register` and `POST /api/v1/auth/login` are removed.
- `/api/v1/auth/mode` no longer returns `passwordLoginEnabled` or `setupRequired`.
- The database migration drops `rootcx_system.users.password_hash`. It preserves
  user IDs, OIDC identities, role assignments, sessions and application data.
- The environment variable `ROOTCX_DISABLE_PASSWORD_LOGIN` is no longer used.

## Before upgrading

Configure and verify an OIDC provider, including access for workspace
administrators. Standalone Core accepts `ROOTCX_OIDC_ISSUER`,
`ROOTCX_OIDC_CLIENT_ID` and `ROOTCX_OIDC_CLIENT_SECRET`. Register the Core callback
at `/api/v1/auth/oidc/callback` with that provider.

Take a database backup and preserve the Core data directory, including signing
keys and deployed application files. Users who only had local passwords need a
working SSO or governed invitation path before the upgrade.

Update frontends and clients that still call the removed routes. The SDK and
scaffold changes in this repository remove the local form and automatically use
a sole SSO provider, retaining provider selection and manual retry. They require
their own package release and app rebuild; publishing the Core image alone does
not change an already deployed frontend.

## After upgrading

Verify Core readiness, SSO sign-in, existing users and roles, session refresh,
and an application read/write operation. The removed routes should return 404.

## Rollback

Do not run an older Core binary against the migrated database as a rollback.
The old password hashes cannot be recovered from the upgraded database, and
the older binary does not contain the new migration.

Restore the database backup and matching Core data directory, then start the
previous image. Coordinate writes before restoring so that post-upgrade work
is not silently discarded.

## Release scope

`core-v0.29.0` publishes the Core container. SDK, CLI and crates.io packages are
not published by the Core-only release procedure. Existing cloud tenants are
upgraded separately.
