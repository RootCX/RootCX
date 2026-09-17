# Governed row access

This guide describes the next release; the release version has not been bumped.
Core enforces row access with PostgreSQL RLS and sensitive-field access with
database column privileges. The rules apply to generated collection operations,
worker SQL, and callback transactions.

## Ownership stays separate

An `owner: true` field identifies the Core user who owns a row. It can hold that
identity directly (`uuid`, `text`, or an `entity_link` to `core:users`) or link to
another locally owned entity. Delegated ownership follows those existing links
to a Core identity. A chain may span at most four entities, cannot loop, and
must end in a direct owner. A NULL owner belongs to nobody.

`app:{app}:{entity}.{action}.own` grants the corresponding operation on the
caller's own rows. Assignment sharing does not change owner definitions or
transfer ownership. Unscoped permissions remain separate broader grants.

## Share an exact resource

Use `scope: "resource"` when access should stop at a particular project, ticket
or other app row. The subject needs no Owner or Core identity. For example:

```json
{
  "appId": "projects",
  "name": "Projects",
  "dataContract": [
    { "entityName": "member", "fields": [
      { "name": "user_id", "type": "entity_link", "owner": true,
        "references": { "entity": "core:users", "field": "id" } }
    ] },
    { "entityName": "project", "fields": [
      { "name": "name", "type": "text" }
    ] },
    { "entityName": "folder", "fields": [
      { "name": "project_id", "type": "entity_link",
        "references": { "entity": "project", "field": "id" } }
    ] },
    { "entityName": "document", "fields": [
      { "name": "folder_id", "type": "entity_link",
        "references": { "entity": "folder", "field": "id" } },
      { "name": "body", "type": "text" }
    ] },
    { "entityName": "assignment", "share": {
      "scope": "resource",
      "grantee": "member_id",
      "subject": "project_id",
      "activeWhen": { "isNull": "end_date" },
      "targets": [
        { "entity": "document", "via": ["folder_id", "project_id"] }
      ]
    }, "fields": [
      { "name": "member_id", "type": "entity_link",
        "references": { "entity": "member", "field": "id" } },
      { "name": "project_id", "type": "entity_link",
        "references": { "entity": "project", "field": "id" } },
      { "name": "end_date", "type": "date" }
    ] }
  ]
}
```

An active assignment shares that exact project and its documents. Grant
`app:projects:project.read.shared` and/or
`app:projects:document.read.shared` as needed. Granting only document reads works
without read grants on members, assignments, folders or projects. Folders are
used for resolution but are not themselves shared. Other projects remain
private through this declaration even when they have the same Owner.

Each `via` path starts on its target entity and follows one to three local
primary-key `entity_link` fields to the Subject entity. Paths cannot repeat an
entity. At most 32 paths are allowed per assignment declaration; distinct paths
to the same target entity combine. The Subject itself is always a possible
target, so omit `targets` when sharing only that row. Do not add an empty path
for the Subject. Keys used in paths and returned target primary keys must be
nonsensitive.

The Grantee must still link to a locally owned entity. All read authority comes
from live relationships and exact target permissions. No app data is duplicated,
no other share is followed, and ownership links do not implicitly add targets.
Control changes to assignments **and** intermediate relationship fields: moving
a document into a shared folder changes which readers can access it.

This mode grants row access on every governed read path, including direct HTTP
and SQL. It does not provide per-reader field redaction, shared writes or a
worker-only permission. Sensitive fields and independent broader grants retain
their existing meaning.

For the complete setup with an administrator, a sharing manager and two readers,
follow [Resource sharing walkthrough](resource-sharing-walkthrough.md). It covers
Core identities, local members, role grants, initial records, reads and revocation
using the public HTTP operations.

## Share an identity's records (legacy default)

Omitting `scope`, or explicitly setting `"scope": "identity"`, retains the
original identity-wide sharing behavior below. Resource `targets` are not
accepted in identity mode. Changing to resource mode is an explicit contract
change, never a silent upgrade.

This minimal assignment entity assumes that a local `enrollment` entity already
has an owner, directly or through a valid ownership chain:

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

The grantee and subject must be different fields linking to owned entities in
the same app. They may reference the same entity, as above. `activeWhen.isNull`
names a nullable `date` or `timestamp` field. It accepts a column name, not a
SQL expression.

Core resolves both links through the existing owner chains:

1. The owner of the grantee enrollment must be the caller's Core identity.
2. The subject enrollment resolves to the helped person's Core identity.
3. For each authorized target entity, Core returns rows owned by that subject
   identity, including rows reached through delegated ownership.

For example, suppose one person owns two enrollments, a profile, and notes
linked through either enrollment. An active assignment referencing the first
enrollment can expose both enrollments, the profile, and those notes. Each
target still needs its own `.read.shared` permission. Sharing is by subject
Core identity, not by the subject enrollment's primary key.

Sharing does not recurse: if the helped person can read a third person's records
through a separate assignment, that access is not inherited by the helper.
These declarations do not authorize access across apps.

## Grant only the required target reads

For an app named `support`, grant the relevant target permissions explicitly:

```text
app:support:enrollment.read.shared
app:support:profile.read.shared
app:support:note.read.shared
```

Granting only `note.read.shared` exposes shared notes, not profiles or
enrollments. The caller does not need read permissions on the assignment or
intermediate ownership entities for Core to resolve a shared read. `.read.shared`
is a Core-generated scope; do not redeclare it as a custom permission.

| Permission | Rows or operations it authorizes |
| --- | --- |
| `note.read.own` | Notes owned by the caller |
| `note.read.shared` | Resource mode: explicitly shared notes or notes reached by declared target paths from active assignments. Identity mode: notes owned by active assignments' subject identities |
| `note.read` | Broader note reads under the app's normal unscoped policy |
| `note.update.own` | Updates confined to the caller's own notes |

Multiple applicable read grants combine. `.read.shared` alone does not grant
`.own` reads or any write operation. There are no `.create.shared`,
`.update.shared`, or `.delete.shared` scopes.

Creating or changing assignments changes authorization data. Give the required
assignment create/update/delete permissions only to explicitly authorized
principals. A shared-read grant never grants permission to fabricate an
assignment, change an owner, or reparent a record. Scoped updates remain checked
against both the old and new ownership.

## Revocation

An assignment is active only while `end_date IS NULL`. Any non-NULL value,
including a future date, makes it inactive; the field is not an expiry timer.
Deleting the assignment also removes that route to access.

Governed SQL uses `READ COMMITTED`. If a revocation commits before the next
statement begins, that statement observes it, even inside
`ctx.transaction(async tx => ...)`. A statement already running retains its
statement snapshot, and data already returned cannot be retracted. Another
active assignment or an independent broader grant may still authorize a read.

## Query plans and performance

Read access combines permission-gated table-wide, `.own`, and `.shared`
branches with OR. An unfiltered SELECT can therefore scan the target table even
when only a small shared set is visible. The 200,000-row performance regression
exposed this behavior; an ownership index does not guarantee an indexed target
scan.

The shared branch uses `column IN (SELECT rootcx_shared...)` membership. The
regression checks that PostgreSQL builds a `hashed SubPlan` once and does not
run the resolver separately for each target row. Assignment lookups have
Core-generated indexes. This bounds repeated membership work; a target
`Seq Scan` may still be part of the plan. There is no universal index-only or
always-indexed performance guarantee.

A single allowed-key array for all read branches was rejected because a
`STABLE` resolver snapshot missed brand-new ownership values and broke
authorized `INSERT ... RETURNING`. The final design preserves the separate
branches and write behavior. See [ADR 0006](adr/0006-governed-row-access.md) for
the tradeoff.

## Sensitive fields also restrict SQL

Mark a field with `"sensitive": true` to withhold its `SELECT` privilege from the
worker's restricted database role. Generated reads omit it and reject filtering
or sorting by it. PostgreSQL also denies explicit SQL references, including
expressions, predicates, joins, ordering, and `RETURNING`.

For a table `support.note` with safe columns `id`, `body`, and `enrollment_id`,
and a sensitive column `private_reference`:

```sql
-- Can succeed when RLS permits the rows.
SELECT id, body FROM support.note;
UPDATE support.note SET private_reference = $1 WHERE id = $2 RETURNING id;

-- Denied by column SELECT privileges.
SELECT * FROM support.note;
SELECT lower(private_reference) FROM support.note;
SELECT id FROM support.note WHERE private_reference = $1;
UPDATE support.note SET body = $1 WHERE id = $2 RETURNING private_reference;
UPDATE support.note SET body = $1 WHERE id = $2 RETURNING *;
```

Writes can set sensitive values if RLS allows the operation, but cannot read
them back or use their existing values in an expression. Replace wildcard
projections with explicit safe columns. A broad app permission does not override
database column privileges. The flag is not encryption and does not constrain a
trusted database administrator.

An app declaring sharing cannot mark an ownership field `sensitive`. Shared
resolvers return ownership keys and are callable by the restricted executor;
column privileges alone cannot conceal those return values. Core refuses this
combination at installation. Other fields can remain sensitive.

## Contract lifecycle and SQL admission

The Core row-access module owns versioned `row_access_contracts` and the
`sensitive_fields` projection used by audit and hooks. It atomically reconciles
these projections with ownership/sharing resolvers, RLS policies, and column
privileges per app. Legacy manifests are validated before first migration into
the contract; later boots validate and replay the versioned contract. Resource
sharing uses version 2; identity-only and nonsharing contracts remain version 1.
Core refuses a resource declaration stored as version 1. Old Core binaries do
not support version 2.

Ownership validation lives in `core/src/governance/row_access/ownership.rs`.
The old ownership names in `manifest.rs` are compatibility aliases into that
module; owner declarations and `.own` behavior have not changed.

Use declarative fields, enums, supported column-based indexes, and ownership or
sharing declarations. Raw manifest checks, index expressions, partial-index
`where`, operator-class `ops`, and index `with` parameters are refused for now.
Core-generated indexes remain supported, including the partial index generated
from `activeWhen.isNull`.

Unexpected legacy database routines, views, triggers, rules, or other
unsupported artifacts require an administrator-controlled migration. Admission
does not execute app SQL to repair them and cannot certify a compromised
historical Core/global database.

`onStart` receives no implicit data authority. Its collection and SQL operations
use the lifecycle worker's fixed no-user identity through governed transactions.
Move data initialization to an explicitly authorized operation. Pending backend
SQL migration files are refused without execution or bookkeeping; use the
manifest or an independently controlled administrator migration.

Dependency installation does not inherit the Core environment or run package
lifecycle scripts, including trusted dependency scripts. Build required
generated/native artifacts in a controlled pipeline before uploading the archive.

See [ADR 0006](adr/0006-governed-row-access.md) for the decision and
[the migration guide](migration-v027.md) for operator preparation.
