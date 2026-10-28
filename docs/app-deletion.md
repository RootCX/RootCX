# Application deletion contract

Ordinary applications use an installation-bound operation. This Core capability
uses native RBAC, AgentAuthority, approvals, lifecycle locks and audit. It requires
no product flag, companion, portal, external product service or branded agent.
Source projects and change history belong to Core's existing Builder module.

## HTTP callers

1. `POST /api/v1/apps/{appId}/deletion` checks `admin:apps.delete` and returns the
   operation ID, canonical application name and installation ID without deleting.
2. Show the canonical target and consequences, then obtain one confirmation.
3. `POST /api/v1/app-deletions/{id}/confirm` revalidates authority and admits once.
4. Inspect `GET /api/v1/app-deletions/{id}` until `succeeded`. After a lost response,
   inspect the same operation; do not assume either success or failure.

Only admission requires current deletion authority. An admitted operation remains
a Core cleanup obligation after revocation or restart. Its audit records the
human and any executing agent. A receipt expires after ten minutes before admission.

## Compatibility

- Ordinary-app `DELETE /api/v1/apps/{appId}` returns 409 after authorization. Do
  not fall back to this route when prepare/confirm fails.
- Registered agents retain their previous DELETE route and `admin:apps.install`
  permission. Registration is checked again under the lifecycle lock. This path
  cannot remove an ordinary app or discard a managed source project.
- Integrations use `DELETE /api/v1/integrations/catalog/{id}`. Non-integration IDs
  are refused. Existing connection, collection, secret and cron DELETE routes
  retain their contracts.
- Rust `RuntimeClient::uninstall_app` retains its signature, deprecated for
  ordinary apps. Use `prepare_app_deletion`, `confirm_app_deletion`, `app_deletion`
  for applications and `uninstall_integration` for integrations.
- `rootcx apps rm` selects the lifecycle by the registered target type and keeps
  one user confirmation (`--yes` is explicit consent). The generic cloud portal
  uses prepare/confirm for apps and the existing route for agents.

This is deliberately not wire-compatible for unknown external clients deleting
ordinary apps. Those consumers must migrate before upgrading; repository searches
and local tests cannot establish the absence of external clients.

## Local verification

Real Core/PostgreSQL tests exercise legacy agent permissions/cleanup, concurrent
kind changes, app confirmation, denial, expiry, reinstall and recovery. CLI tests
capture HTTP requests for apps, agents and integrations and reject fallback after
403/409/500. Browser tests exercise the actual generic portal pages, cancellation,
canonical target confirmation, errors and deferred removal on success.

No deployment or commit is part of this change.
