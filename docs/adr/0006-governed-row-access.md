# ADR 0006: Governed row access

Status: Accepted for Core v0.27.0.

[ADR 0007](0007-resource-sharing.md) adds explicit resource scope. This ADR's
identity scope remains the default for existing declarations.

## Context

Applications need to share another person's records through assignments while
preserving existing ownership rules. Sensitive fields must also remain unreadable
when an app uses SQL directly. Neither property can depend on an app choosing a
generated read API.

Historically, lifecycle collection operations and app-supplied migrations could
use Core's database-owner authority. Those paths could undermine policies and
column permissions. Governance metadata, SQL admission, and database
reconciliation therefore belong to one Core-owned module.

## Decision

### Ownership and assignment sharing

`owner: true` retains its existing meaning: a direct Core user identity or a
local chain of ownership links terminating in that identity. `.own` permissions
remain separate.

An assignment entity may declare:

```json
{
  "entityName": "assignment",
  "share": {
    "grantee": "helper_enrollment_id",
    "subject": "helped_enrollment_id",
    "activeWhen": { "isNull": "end_date" }
  },
  "fields": [
    {
      "name": "helper_enrollment_id",
      "type": "entity_link",
      "references": { "entity": "enrollment", "field": "id" }
    },
    {
      "name": "helped_enrollment_id",
      "type": "entity_link",
      "references": { "entity": "enrollment", "field": "id" }
    },
    { "name": "end_date", "type": "date" }
  ]
}
```

Both links must reference locally owned entities; `enrollment` in this example
must already declare an owner. Existing owner chains resolve the grantee to the
caller and the subject to a Core identity. The shared set is that subject
identity's rows across owned entities in the same app, including sibling
enrollments. It is not limited to descendants of the one referenced enrollment.
Resolving ownership never follows another sharing relation.

Each target requires its own `app:{app}:{entity}.read.shared` permission.
Permission to read one target does not grant access to another target, and
permissions on the assignment or intermediate ownership tables are not required
to resolve the relationship. Sharing adds only a read policy; it grants no
create, update, delete, or assignment-management authority. Existing `.own` and
unscoped permissions retain their meanings.

An assignment is active exactly while its declared nullable date or timestamp
is NULL. A non-NULL future date is already inactive. Governed transactions use
`READ COMMITTED`: a revocation committed before the next statement begins is
visible to that statement, including inside a callback transaction. It does not
retract results already returned or interrupt a statement using an earlier
snapshot.

### Shared membership and query-plan limits

The ordinary read policies combine permission-gated table-wide access OR `.own`
access OR `.shared` access. On an unfiltered SELECT, this union prevents a
guarantee that PostgreSQL will drive the target's ownership index. The
200,000-row performance regression exposed that limitation.

The final shared predicate uses uncorrelated membership:

```sql
enrollment_id IN (
  SELECT rootcx_system."rootcx_shared.support.note"()
)
```

This illustrates the generated membership predicate for a note owned through
`enrollment_id`; the target's `.read.shared` permission gate still applies.
The performance regression checks a `hashed SubPlan` built once, with resolver
execution not repeated for each target row. Core indexes the assignment lookup.
These properties avoid a correlated per-row resolver and repeated linear
searches through the shared set; they do not eliminate scanning the target.
PostgreSQL may choose a `Seq Scan`, and there is no promise of an indexed or
index-only target scan for every query.

**Rejected alternative: one allowed-key array for all read branches.** Unifying
table-wide, own, and shared access into a precomputed key array was intended to
make the target predicate indexable. It broke `INSERT ... RETURNING` for
brand-new ownership values: the `STABLE` resolver's snapshot did not include
those new keys. Preserve authorized write/returning behavior and the separate
policy branches rather than forcing that optimization. Existing ownership
definitions and `.own` semantics are unchanged.

### Sensitive columns

Core reconciles PostgreSQL `SELECT` privileges for `rootcx_app_executor`,
removing table-wide grants that would override column restrictions and granting
only readable columns on tables with sensitive fields. A reference to a
sensitive column is denied in raw SQL projections, expressions, filters, joins,
sorting, and `RETURNING`. `SELECT *` and `RETURNING *` are denied on those
tables; explicit safe-column projections can succeed subject to RLS.

Generated reads omit sensitive columns and reject their use as filter or sort
keys. Writes remain subject to RLS and may set sensitive values; they cannot
read them back through SQL. App RBAC grants, including broad grants, do not
override the executor's column privileges. This is an access-control contract,
not encryption or a restriction on a trusted database administrator.

Ownership fields must be nonsensitive in an app declaring sharing. Shared
resolvers return ownership keys and are executable by the restricted role;
column privileges do not conceal function return values. Reject the conflicting
declaration at installation instead of allowing a direct resolver call to reveal
a sensitive key. This restriction does not change apps without sharing.

### One governance module

`core/src/governance/row_access` owns the validated, versioned
`rootcx_system.row_access_contracts` projection and the
`rootcx_system.sensitive_fields` projection consumed by audit and hooks.
Ownership resolvers, sharing resolvers, column privileges, and RLS policies are
reconciled with those projections in one transaction per app.

Ownership interpretation and validation now live in
`core/src/governance/row_access/ownership.rs`. The old ownership names in
`manifest.rs` remain compatibility aliases into governance; they are not a
second implementation or validation authority.

An older installation without a versioned contract must pass validation and SQL
admission before its stored manifest is migrated into the contract. Subsequent
boots validate and replay the versioned contract through the same reconciliation
path. Malformed or unsupported contracts and unexpected SQL artifacts fail closed;
reconciliation is not a promise that an entire deployment or fleet upgrade is
atomic.

Applications provide bounded declarations, not authorization SQL. Admission
rejects raw manifest `checks`, index expressions, partial-index predicates,
operator-class specifications, and index `with` storage parameters for now.
Supported column-based indexes, field enums, and Core-generated indexes remain
available, including Core's generated active-assignment index.

Admission inspects catalog metadata before operating on app relations.
Unexpected legacy routines, views, triggers, rewrite rules, policies, executable
defaults, and other unsupported artifacts require an independently controlled
administrator migration. A familiar Core object name is not proof that its
contents are trusted. Recognized policy slots are regenerated atomically.

### No implicit lifecycle or migration authority

Every worker collection operation uses its fixed identity and the existing
governed transaction constructor, including lifecycle and anonymous workers.
`onStart` controls hook execution only. Its default identity carries no user or
permissions, and receives no implicit admin or user grants.

Pending app migration files are refused before dependency installation and
startup. Their SQL is never executed or silently marked applied. Archives with
no pending files, including files already recorded in a trusted legacy ledger,
can still deploy. Worker starts also refuse pending files left by a failed
deployment. Schema changes use the declarative manifest or an independently
authorized administrator-controlled database migration.

Dependency installation clears the inherited environment and disables package
lifecycle scripts, including explicitly trusted dependency scripts. Otherwise an
archive could regain Core process authority through `package.json`. Build any
required generated or native artifacts in a controlled pipeline before upload.

## Consequences and limits

Apps using startup seeding, sensitive `SELECT *`, or owner-executed SQL files
need an explicit migration. Operators must inventory SQL artifacts, take and
verify backups, migrate unsupported objects, and verify confined access before
restoring traffic. No upgrade step silently grants permissions to make an app
work.

Admission cannot certify a historically compromised Core database. Earlier owner
migrations could have modified global roles, extensions, Core routines, or
objects outside an app schema. If trusted provenance cannot be established,
restore or rebuild from a trusted baseline and review data import separately.
Passing app-schema admission is not a substitute for that recovery.

See [row-access usage](../row-access.md), the
[next-release migration guide](../migration-v027.md), and
[ADR 0002](0002-governed-worker-transactions.md). This decision preserves the
owner definitions in [ADR 0003](0003-delegated-row-ownership.md) while superseding
its historical projection-replay and implementation-location details. Its
ownership-index discussion is not a target-scan guarantee for the combined
read policies described here. Earlier lifecycle bypass and generated-read-only
sensitive-field guidance is also superseded.
