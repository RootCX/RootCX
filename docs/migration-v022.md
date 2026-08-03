# Migration Guide: v0.22 (trigger + transcript authorization)

Five endpoints that were reachable by any authenticated user now require a
permission, and two row-level triggers stop copying fields marked `sensitive`.

## Upgrading an existing tenant

Nothing to do. The upgrade is additive:

- Every new permission key is minted the next time an app is installed or
  deployed. Until then only `*` / `app:{id}:*` holders pass the new gates.
- The sensitive-field projection (`rootcx_system.sensitive_fields`) stays empty
  until an app redeploys, and an empty strip list is a no-op — so an unredeployed
  tenant's hook payloads and audit rows are byte-identical to before.

Nothing in Studio, the SDK, the Rust client or the CLI calls the newly-gated
endpoints, so no first-party client breaks. If you drive these endpoints from
your own scripts, read on.

## Permission gates added (previously open to any authenticated user)

| Action | Before | After | Required permission |
|--------|--------|-------|---------------------|
| List entity hooks | Any user | Own hooks only | `app:{id}:hook.read` |
| Register an entity hook | Any user, any app's table | Requires the grant | `app:{id}:hook.write` |
| Read one entity hook | Any user, anyone's hook | Own hooks only | `app:{id}:hook.read` |
| Delete an entity hook | Any user, anyone's hook | Own hooks only | `app:{id}:hook.write` |
| Read an agent transcript (`/agent/sessions/{id}/events`) | Any user | Admin only | `admin:agents.manage` |
| Subscribe to the agent event stream (`/agents/stream`) | Any user | Admin only | `admin:agents.manage` |

Reaching a hook you do not own additionally requires
`app:{id}:hook.manage_others`. Reading or deleting someone else's hook without it
answers `404`, not `403`, so the endpoint does not confirm that an id exists.

`app:{id}:cron.manage_others` was already enforced but never minted, so it could
not be granted. It is minted now, alongside the hook keys.

### If you call these endpoints from your own code

Grant the keys to a role your caller holds:

```
PATCH /api/v1/roles/{role_name}
{ "permissions": ["app:myapp:hook.read", "app:myapp:hook.write"] }
```

The keys exist once `myapp` has been installed or deployed on the upgraded Core.
Before that, use an admin token.

## Trigger confinement

- **A manifest-declared trigger may only watch its own app.** `trigger.appId` was
  bound verbatim, and installing an app is self-service, so an app could plant a
  hook — and therefore an agent prompt — on another app's rows. A mismatch now
  fails the install with a clear error.
- **Manifest triggers now carry an owner.** They were inserted with
  `created_by = NULL`, which `assert_can_fire` denies at dispatch: the declared
  trigger silently never ran. The installer is recorded as the owner, so it fires.
  Redeploy to fix an existing ownerless trigger.
- **A workflow's record-change trigger requires read access to what it watches.**
  A workflow legitimately watches another app, so it cannot simply be pinned to
  its own; instead enabling it now requires `app:{app}:{entity}.read` on the
  target. Owning the workflow is no longer sufficient.

## Sensitive fields in triggers

A field marked `"sensitive": true` was already excluded from every generated read
path. It was still copied whole by the two row-level triggers — into job
payloads, into agent prompts, and durably into `audit_log`. Both now consult
`rootcx_system.sensitive_fields`, a per-entity projection synced from the manifest
at deploy.

Redeploy an app to populate the projection. Until you do, its triggers behave
exactly as before.

Performance note: the hooks trigger builds its payload inside the loop over
matching hooks, so a table with no hook registered now does no lookup and no row
serialization — cheaper than before. The audit trigger adds one indexed
primary-key lookup per audited write.

## What this does not change

- `ctx.sql` still reads sensitive columns. The flag governs generated surfaces,
  not the app's own SQL. It is not encryption and not an RLS guarantee.
- Hooks with no owner remain reachable through the API for their app; they are
  refused at dispatch instead.
