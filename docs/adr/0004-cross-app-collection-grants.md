# ADR 0004: Explicit cross-application collection grants

Status: Accepted

## Context

User RBAC alone does not express which application may consume another
application's data. Shared schemas and entity links must not implicitly create
that relationship, and a schema update must not silently widen approved access.

## Decision

Core authorizes cross-app CRUD through explicit grants tied to both installation
generations, a provider collection, exact operations, and separate frozen
readable and writable field scopes. Core derives the consumer identity; request
payloads cannot supply it. Provider RLS, ownership, and delegated permission
ceilings still apply.

Authorization, transaction rechecks, projection, and audit share one operation
path for workers and tools. Revocation and execution synchronize on the same
grant lock. Successful mutations and their audit commit together.

Governance-contract changes invalidate installation authority before exposing
the new contract. Cosmetic metadata updates retain grants; failed contract
updates do not restore revoked authority. Uninstall/reinstall never revives old
grants. Revoked and expired grants are terminal.

Local and remote collections use the same CRUD conventions. `ctx.remote`
makes provider selection explicit; it is not itself a security control.
`find` preserves the complete equality-array result, while `findPage` makes
pagination explicit instead of silently truncating existing calls.

## Consequences

- Consumers need approved grants in addition to applicable delegated permissions.
- Frozen field scopes prevent future schema additions from widening access.
- Contract changes require fresh approval; failed changes may require recovery.
- Each remote operation has its own provider transaction; there is no distributed
  transaction or implicit cross-schema SQL access through a grant.

API examples, lifecycle details and migration steps:
[Cross-app collections](../cross-app-collections.md).
Contributor checks: [Core testing](../testing.md).
