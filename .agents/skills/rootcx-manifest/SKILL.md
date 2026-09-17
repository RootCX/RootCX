---
name: rootcx-manifest
description: Writing or editing manifest.json for a RootCX app — defining the data contract, entities, field types, entity links, RBAC permissions, and understanding how Core syncs the schema to PostgreSQL on install/deploy.
version: 0.1.0
---

# RootCX App Manifest

Apps require: `manifest.json` (data contract) + React code using `@rootcx/sdk` hooks and `@rootcx/ui` components.

The governed row-access guidance below targets the next release; the release
version has not been bumped. See [row access](../../../docs/row-access.md) and
[operator migration](../../../docs/migration-v027.md).

## manifest.json

```json
{
  "appId": "<id>",
  "name": "<Name>",
  "version": "0.0.1",
  "description": "<description>",
  "dataContract": [
    {
      "entityName": "<entity>",
      "fields": [
        { "name": "<field>", "type": "<type>", "required": true },
        { "name": "<field>", "type": "entity_link", "references": { "entity": "<target>", "field": "id" } },
        { "name": "<field>", "type": "text", "enum_values": ["a", "b", "c"] }
      ]
    }
  ],
  "permissions": {
    "permissions": [
      { "key": "<entity>.<action>", "description": "<description>" }
    ]
  }
}
```

### Field types

`text` `number` `boolean` `date` `timestamp` `json` `file` `entity_link` `[text]` `[number]`

### Approved action authority

Declare an action's exact local data ceiling separately from user permissions:

```json
{
  "id": "add_interaction",
  "name": "Add an interaction",
  "authority": {
    "data": {
      "assignment": ["read"],
      "interaction": ["read", "create"]
    }
  }
}
```

This is one `actions[]` entry; declare both entities in `dataContract`.
Use lowercase snake_case IDs consistently in handlers, grants and calls;
permission-key validation rejects uppercase. API/body keys remain camelCase.
Allowed verbs are `read`, `create`, `update`, `delete`, with nonempty unique
lists and exact local entity names. No wildcards, external entities or policy
DSL. An authority action cannot also be a public RPC. `isolatedScope` alone
never grants data authority.

The administrator approves the full manifest and backend artifact, including
resolved dependencies and every handler sharing its runtime. Core hashes after
dependency installation and records a versioned snapshot, verifying its hash
at approval and approved-process startup. Read-only settings are defensive,
not immutability against hostile processes under the same OS account.
Every backend deployment and every full manifest change, including cosmetic changes, rotates
the release revision and invalidates approval. Old approvals never revive.
The current API pins exact review with `revision`, `backendDigest` and
`installationId`; no extra `manifestDigest` is needed.

Grant ordinary users only `app:{appId}:action:{id}` for the workflow. Approved
calls require that fine grant plus current approval; `invoke` alone is
insufficient. Ordinary declared actions without authority accept `invoke` OR
the fine action grant; undeclared RPCs require `invoke`. Declaration/deployment
does not approve actions or grant users collection permissions.
Invocation without current approval returns HTTP 403; a stale approval POST
returns HTTP 409.

Sensitive reads remain forbidden and caller data grants do not widen the
approved ceiling. The per-approval PostgreSQL role and RLS enforce denied
table/verb access with database errors. Foreign-key cascades,
set-null and set-default effects require explicit authority for all affected
local entities/verbs or approval is rejected; cross-app effects are refused.
Approved contexts support only local CRUD, with no jobs,
remote/self-action calls, tools, integrations, storage, events or credentials.
Approved execution uses the usual cross-platform Bun spawning path, with a
fresh process per invocation that is stopped after use. It adds no OS-identity
or volume-ownership prerequisite. Ordinary apps remain untrusted at governed
IPC/SQL boundaries; the full approved artifact is trusted for business checks.
Host filesystem/process confinement is outside the existing deployment model.
Do not infer complete malicious-app confinement or tested platform coverage
from this architecture.

See the [complete manifest, worker and operator recipe](../../../docs/approved-actions.md).
It keeps one case for two readers, hides private notes through explicit
projections and creates under an active assignment lock in `ctx.transaction`.
Assignment `read` alone permits `SELECT ... FOR SHARE`; do not add `update`
for locking. Core's technical primary-key UPDATE ACL and RLS `USING` support
the lock, but `WITH CHECK` denies updates without declared `update`, including
primary-key changes. `interaction.read` permits `RETURNING`. Assignment
revocation follows app transaction rules; Core approval revocation separately
waits for admitted database transactions.

### Rules

- `id`, `created_at`, `updated_at` are auto-generated — omit from `fields`
- `entity_link` requires `"references": { "entity": "<target>", "field": "id" }`. `<target>` is `"<entity>"` (same app) or `"core:users"` (FK → `rootcx_system.users`, `ON DELETE SET NULL`). Cross-app refs not yet supported.
- `"required": true` = mandatory on create; omit key for optional
- `"enum_values": [...]` restricts text fields to fixed values
- `"sensitive": true` omits a column from generated reads and rejects generated filter/sort references. Core also withholds the worker executor's PostgreSQL column `SELECT` privilege, so raw SQL references, expressions, filters, and `RETURNING` cannot read it. `SELECT *` and `RETURNING *` fail on tables containing sensitive columns; explicitly project safe columns. Writes may set sensitive values subject to RLS, but cannot read their existing values or return them. Broad app RBAC grants do not override column privileges.
- `"owner": true` marks the column deciding which user a row belongs to. Core then mints `{entity}.{action}.own` permissions next to the unscoped ones, and a role holding only those reaches that user's own rows. One column per entity. Rows whose owner is NULL belong to nobody, so adopting this on an existing table needs no backfill. It scopes rows, not columns: a confined caller reads its own row whole, so mark credential columns `"sensitive": true` too.
  - **Direct** — a `uuid`, `text`, or `entity_link` to `core:users` column holding the user id itself.
  - **Delegated** — an `entity_link` to another entity of the same app: the row belongs to whoever owns the row it links to. That entity must be marked `"owner": true` in turn, so the chain ends on a real user id. This is how a table that stores no user id anywhere still has owners.

```json
{ "entityName": "enrollment", "fields": [
    { "name": "user_id", "type": "uuid", "owner": true }] },
{ "entityName": "assignment", "fields": [
    { "name": "enrollment_id", "type": "entity_link",
      "references": { "entity": "enrollment", "field": "id" }, "owner": true }] },
{ "entityName": "submission", "fields": [
    { "name": "assignment_id", "type": "entity_link",
      "references": { "entity": "assignment", "field": "id" }, "owner": true }] }
```

  A role holding `app:school:submission.read.own` now sees the submissions hanging off its own enrollments, and no others. Ownership is resolved from the data alone, so grants on `assignment` or `enrollment` neither widen nor narrow it. Refused at install: a chain that loops, one spanning more than four entities, one ending on an entity that declares no owner, and an `entity_link` owner with no `references`. Core indexes link columns to support ownership resolution (`entity_link` already is indexed); the complete RLS predicate and query determine the actual scan plan.

### Assignment sharing

For an exact project, ticket or other resource, opt into resource mode:

```json
{
  "grantee": "member_id",
  "subject": "project_id",
  "scope": "resource",
  "activeWhen": { "isNull": "end_date" },
  "targets": [
    { "entity": "document", "via": ["folder_id", "project_id"] }
  ]
}
```

This is the assignment entity's `share` value. Its two fields must be local
`entity_link`s to primary keys. The member must have a valid Owner; the project
needs no Owner or Core identity. Declare all referenced entities and fields.
The project itself and the named document target get `.read.shared` keys.
Grant each key explicitly; intermediate folders do not become readable.
`via` follows links from document to folder to the exact project.

Paths contain 1–3 links, cannot repeat an entity, and must end at the subject
entity. At most 32 distinct paths are allowed per declaration. Path keys and
returned primary keys must be nonsensitive. Omit `targets` to share only the
subject row. Ownership does not imply additional targets; no records are copied
and no other sharing relation is followed. Govern modifications to assignment
rows and intermediate links, since both change access.

This governs rows on HTTP and SQL alike; it does not provide per-reader field
redaction or shared writes. Resource contracts use version 2 and require the
next-release Core implementing this mode; the release version is not yet bumped.

**Legacy identity mode:** omitting `scope` (or using `"identity"`) retains the
behavior below. Nonempty `targets` require resource mode. Never silently change
an existing app's sharing scope.

Keep existing owner definitions intact. An assignment can separately declare
shared reads:

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

Both links must target local entities with valid owners; `enrollment` above
must already declare ownership. The fields must be distinct, and `end_date`
must be a nullable date or timestamp. No SQL condition is accepted.
Ownership fields must be nonsensitive in an app declaring sharing: Core refuses
`owner: true` combined with `sensitive: true`, because callable shared resolvers
return ownership keys. Keep confidential values in separate fields.

Core follows existing owner chains to match the grantee to the caller and
resolve the **subject's Core identity**. Shared reads reach that identity's rows
across owned entities in the same app, including sibling enrollments, only where
the caller holds the target's `app:{app}:{entity}.read.shared` permission. They
are not restricted to descendants of the one referenced subject row. Sharing
never follows another sharing relation, and source/intermediate-table read
grants are not required for resolution.

`.own` remains separate. `.read.shared` grants no create, update, delete, owner
change, or assignment-management authority. Do not declare custom permissions
ending in `.shared`; Core generates the supported target read keys. Provision
target read and assignment-management permissions explicitly; never compensate
for missing authorization with automatic admin/user grants.

Only NULL means active. Any non-NULL `end_date`, including a future date,
revokes that assignment. Under governed `READ COMMITTED` transactions, a
revocation committed before the next statement begins is visible to that
statement, including inside `ctx.transaction`. Other independent grants may
still allow access.

### Performance limits

The table-wide OR `.own` OR `.shared` read-policy union may cause a target
`Seq Scan` on an unfiltered SELECT, as exposed by the 200,000-row regression.
Shared membership uses `column IN (SELECT rootcx_shared...)`; the regression
checks a `hashed SubPlan` built once, with no correlated per-row resolver.
Assignment lookups are indexed. Do not promise an indexed or index-only scan
of every target query.

Do not unify all read branches into one precomputed allowed-key array: that
approach broke authorized `INSERT ... RETURNING` for brand-new ownership values
because a `STABLE` resolver's snapshot lacked the new keys. Keep ownership and
the separate policy branches intact.

---

## Schema Sync

Core validates declarations and performs SQL admission before applying schema
changes. For admitted manifests, schema sync creates missing schemas/tables and
reconciles supported fields, types, nullability, literal defaults, enums, and
indexes. The row-access governance module owns versioned
`rootcx_system.row_access_contracts` and the `sensitive_fields` projection used
by audit/hooks, reconciling these with resolvers, column privileges, and RLS
atomically per app. Legacy stored manifests are validated before migration;
later boots validate and replay the versioned contract.

Ownership interpretation and validation live in
`core/src/governance/row_access/ownership.rs`; old `manifest.rs` ownership names
are compatibility aliases into governance, not a second validation module.

Raw manifest `checks`, index expressions (`expr`), partial-index predicates
(`where`), operator classes (`ops`), and index storage parameters (`with`) are
refused for now. Use field enums and supported column-based indexes.
Core-generated indexes remain supported, including active-assignment indexes;
do not copy their SQL into a manifest or backend migration file.

Unexpected legacy routines, views, triggers, rules, or other unsupported SQL
artifacts require an independently controlled administrator migration. App
SQL is not run to repair admission failures. Admission cannot certify a
historically compromised Core/global database; operators must establish trusted
provenance or restore/rebuild from a trusted baseline.

`onStart` has no implicit data authority: lifecycle and anonymous collection
operations use the worker's fixed identity through governed transactions.
Initialize data through an explicitly authorized operation. Pending
`backend/migrations/*.sql` files cause deployment/start refusal without execution
or being marked applied. Archives with no pending files remain deployable,
subject to admission. Use the declarative manifest or an independently
authorized administrator migration for required changes.

Backend uploads install dependencies without package lifecycle scripts, including
root-package and explicitly trusted dependency scripts, and without inheriting
the Core environment. Build required generated/native artifacts in a controlled
pipeline before upload.

### Manifest ↔ DB contract

`dataContract` fields map to columns. Auto-columns (`id UUID`, `created_at`, `updated_at`) added by Core — omit from manifest `fields`. Type mapping: `text`→`TEXT`, `number`→`DOUBLE PRECISION`, `boolean`→`BOOLEAN`, `date`→`DATE`, `timestamp`→`TIMESTAMPTZ`, `json`→`JSONB`, `file`→`TEXT`, `entity_link`→`UUID`, `[text]`→`TEXT[]`, `[number]`→`DOUBLE PRECISION[]`.
