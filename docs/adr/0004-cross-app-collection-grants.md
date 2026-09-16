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

## Amendment: public collection reads

A public execution additionally requires a provider-approved publication bound
to the installation generations, exact fields, fixed row predicate and declared
public entry. Core supplies a managed, credentialless service identity.
The publication never substitutes for a cross-app grant.

An explicit `releaseOwnership` approval grants a separate read authority over
the published rows. A grant alone still cannot bypass ownership. Publication
RLS ceilings apply to every policy branch; other restrictive provider policies
remain effective. Public execution cannot mutate data or acquire ordinary
worker capabilities. Local and remote publications use the same read executor.

Developer contract and approval lifecycle: [Public data](../publications.md).
