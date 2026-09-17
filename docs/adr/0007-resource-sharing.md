# ADR 0007: Share an exact resource without duplicating app data

Status: Accepted for the next release; the release version has not been bumped.

## Context

ADR 0006's identity sharing resolves an Assignment's Subject to its Owner, then
shares that Core identity's rows on permitted target entities. This is useful
when access deliberately follows a person across their records. It does not
express access limited to one project, ticket or document. Requiring a Core
identity for an owner-free resource, or copies of app data per reader, pushes
authorization complexity and revocation synchronization into Apps.

The existing row-access Module already owns validation and transactional
compilation. Extend its Interface rather than introduce a parallel authority.

## Decision

- An Assignment can opt into `scope: "resource"`. Its Subject references the
  exact local resource row, which needs no Owner. Its Grantee continues to
  resolve through an owned local entity to the caller's Core identity.
- The resource itself is readable with its entity's `.read.shared` permission.
  Other readable targets require explicit `targets`, each naming an entity and
  a `via` path from that entity to the resource. The path follows local
  `entity_link` references to primary keys. Ownership does not imply sharing.
- Paths contain one to three links, never repeat an entity, and end at the
  declared Subject entity. Each Assignment has at most 32 target paths.
  Duplicate paths, unknown entities/fields, SQL expressions, non-primary-key
  links and sensitive path keys are refused. Target primary keys must also be
  nonsensitive because callable resolvers return them.
- Intermediate entities require no read grant and do not become shared.
  Paths inspect the live relationships; they never follow another Assignment's
  sharing authority. No app records or per-reader access sets are copied.
- Every target requires its exact `.read.shared` permission. The permission and
  Core-bound app/user checks apply inside the callable resolver as well as RLS.
  HTTP, worker collections and worker SQL enforce the same row access.
- The existing activation condition remains unchanged: only NULL is active.
  Governed `READ COMMITTED` transactions observe a committed revocation at the
  next statement, subject to other independent grants.
- Sharing never grants writes or authority to manage Assignments. Existing Own
  scope and Sensitive field protections retain their meanings.

## Compatibility and compilation

Omitting `scope`, or specifying `"identity"`, retains ADR 0006's identity
semantics, including sibling rows. `targets` is rejected in identity mode.
Existing identity-only declarations serialize as before and retain their
resolver shape. No stored declaration is silently reinterpreted.

Resource resolvers return target primary keys computed from the explicit
resource paths. If identity and resource sharing both reach an entity, the
resolver unions their independently authorized row sets. The Own scope remains
a separate policy branch; the historical `INSERT ... RETURNING` failure from
combining all read authority into one stable snapshot must not recur.

Row-access contracts containing resource sharing use version 2. Version 1
remains supported for identity-only or nonsharing Apps. Core upgrades its
metadata version constraint without changing app data and rejects resource
declarations disguised as a version 1 contract. Removing all resource sharing
returns that App's projection to version 1. Older binaries must refuse version 2
instead of interpreting it as identity sharing.

## Consequences

Apps declare access using their existing relationships and manage those
relationships with explicit permissions. Changing an intermediate relation
changes the set of shared rows; controlling these writes is part of controlling
authorization.

The compiler does more work, but the Implementation and its verification retain
Locality in the row-access Module. There is no universal predicate language,
implicit propagation, cross-app path, per-role column visibility or new shared
write scope in this change. Resource sharing controls rows; it does not redact
different fields for different readers.

Resource-only and mixed queries need their own representative query plans.
The legacy identity benchmark is not evidence of resource-path performance.
Tests exercise the public Interface through real HTTP, Bun and PostgreSQL,
including exact row sets, denied mutations, resolver guards, revocation,
redeploy and bootstrap. The existing identity suite remains a compatibility gate.
