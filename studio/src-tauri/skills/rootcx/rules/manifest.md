# RootCX App Manifest

Apps require: `manifest.json` (data contract) + React code using `@rootcx/sdk` hooks and `@rootcx/ui` components.

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

`text` `number` `decimal` `boolean` `date` `timestamp` `json` `file` `entity_link` `[text]` `[number]`

### Approved action authority

An entry in `actions` can request exact local CRUD authority:

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

Declare these entities in `dataContract`. Allowed verbs are `read`, `create`,
`update`, `delete`; lists must be nonempty and unique. No wildcards, external
entities or policy DSL. Authority actions cannot also be public RPCs.
`isolatedScope` alone does not grant data authority.
Use lowercase snake_case IDs in declarations, handlers, grants and calls;
uppercase is rejected by permission-key validation. API/body keys remain
camelCase.

An administrator must review and approve the full manifest and entire backend
artifact, including resolved dependencies, initialization and every handler
sharing its runtime. Core hashes the installed tree after dependencies and
records a versioned snapshot, verifying its hash at approval and process startup.
Read-only settings are defensive, not immutability against hostile processes
under the same OS account. Every backend deployment and every full manifest
change, even cosmetic, rotates revision and invalidates approval; old approvals
never revive. Exact review is pinned by `revision`, `backendDigest` and
`installationId`, with no extra `manifestDigest`.

Use `rootcx apps actions list <app> --json`, then
`rootcx apps actions approve <app> <action>` to review and confirm.
Noninteractive `--yes` requires `--revision`, `--digest` and `--installation`
from an already reviewed snapshot. Revoke with
`rootcx apps actions revoke <app> <action>`. Deployment never approves.

Users need `app:{appId}:action:{id}` plus current approval, not entity CRUD;
plain `invoke` is insufficient. Ordinary declared actions without authority
accept `invoke` OR the fine action grant; undeclared RPCs require `invoke`.
Invocation without current approval returns HTTP 403; a stale approval POST
returns HTTP 409.
Caller data permissions do not widen approved authority. The per-approval
PostgreSQL role and RLS enforce denied tables/verbs with database errors.
Foreign-key cascades, set-null and
set-default writes require explicit authority for every affected local entity
and verb or approval is rejected; cross-app effects are refused. Sensitive reads
remain forbidden. No jobs, remote/self-action calls, tools, integrations,
storage, events or injected credentials are available.

Implement business checks with `serve`, trusted `caller.userId`, parameterized
`ctx.sql` and `ctx.transaction`. For a guarded create, lock the active business
assignment with `SELECT ... FOR SHARE`, then insert in that same transaction.
Assignment `read` alone permits the lock; do not request `update` for it.
Core provides a technical primary-key UPDATE ACL and RLS `USING` for read locks,
while `WITH CHECK` denies mutations without declared `update`, including
primary-key changes. An explicit `RETURNING` needs target `read`.
Revoke assignments through a separately authorized update/delete of the same
row. Core approval
revocation separately waits for admitted DB transactions. Use explicit output
projections to hide private notes while sharing one case and its interactions
between readers; never duplicate business data per reader.

Approved execution uses the usual cross-platform Bun spawning path with a
fresh process per invocation, stopped after use and never returned to the
ordinary worker cache. It adds no OS-identity or volume-ownership prerequisite.
Ordinary apps remain untrusted at the governed IPC/SQL layer; the full approved
artifact is trusted for business checks. Host filesystem/process confinement is
outside the existing deployment model. Do not claim complete malicious-app
confinement or tested platform coverage without evidence from actual runs.

### Rules

- `id`, `created_at`, `updated_at` are auto-generated — omit from `fields`
- `entity_link` requires `"references": { "entity": "<target>", "field": "id" }`. `<target>` is `"<entity>"` (same app) or `"core:users"` (FK → `rootcx_system.users`, `ON DELETE SET NULL`). Cross-app refs not yet supported.
- `"required": true` = mandatory on create; omit key for optional
- `"enum_values": [...]` restricts text fields to fixed values
- `decimal` is for exact values such as money. `precision` and `scale` are optional, but must be declared together (for example `"precision": 19, "scale": 4`). Decimal values and defaults cross the API as JSON strings so JavaScript never rounds them.
- `"sensitive": true` omits a column from generated reads and rejects generated filter/sort references. Core also withholds the worker executor's PostgreSQL column `SELECT` privilege: raw SQL references, expressions, filters, and `RETURNING` cannot read it. `SELECT *` and `RETURNING *` fail on tables containing sensitive columns; explicit safe-column projections can succeed subject to RLS. Writes may set sensitive values under RLS, but cannot read existing values or return them. Broad app permissions do not override these column privileges.
- `"owner": true` marks the column deciding which user a row belongs to. Core then mints `{entity}.{action}.own` permissions next to the unscoped ones, and a role holding only those reaches that user's own rows. One column per entity. Rows whose owner is NULL belong to nobody, so adopting this on an existing table needs no backfill. Ownership scopes rows; sensitive-column rules apply independently.
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

  A role holding `app:school:submission.read.own` now sees the submissions hanging off its own enrollments, and no others. Ownership is resolved from the data alone, so grants on `assignment` or `enrollment` neither widen nor narrow it. Refused at install: a chain that loops, one spanning more than four entities, one ending on an entity that declares no owner, and an `entity_link` owner with no `references`. Core indexes link columns to support resolution; the complete query and RLS policies determine the scan plan.

### Assignment sharing

Use resource mode for an exact project, ticket or other app row:

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

This is the assignment entity's `share` value. Its endpoints must be distinct
local primary-key `entity_link`s. The member must have a valid Owner; the project
needs no Owner or Core identity. Declare all referenced entities and fields.
The project and explicit document target receive `.read.shared` keys, which
must be granted separately. Folders are traversed without becoming readable.
Each `via` path starts on its target and ends at the subject entity, contains
1–3 local links and never repeats an entity. At most 32 distinct paths are
accepted; path keys and returned primary keys must be nonsensitive.

Omit `targets` for a subject-only share. Ownership never adds implicit targets;
no app data is copied and no other share is followed. Changes to assignments
and intermediate links change authorization and require controlled writes.
Resource sharing applies equally to HTTP and SQL. It grants no writes or
per-reader field redaction. It uses row-access contract version 2 and requires
the next-release Core implementing this mode; no release version is bumped yet.

**Legacy identity mode:** omit `scope` or specify `"identity"` for the existing
behavior below; nonempty `targets` are refused in that mode.

An assignment can separately share a subject's records. This example assumes
`enrollment` is a local entity with a valid owner:

```json
{
  "entityName": "assignment",
  "share": {
    "grantee": "helper_enrollment_id",
    "subject": "helped_enrollment_id",
    "activeWhen": { "isNull": "end_date" }
  },
  "fields": [
    { "name": "helper_enrollment_id", "type": "entity_link",
      "references": { "entity": "enrollment", "field": "id" } },
    { "name": "helped_enrollment_id", "type": "entity_link",
      "references": { "entity": "enrollment", "field": "id" } },
    { "name": "end_date", "type": "date" }
  ]
}
```

- Grantee and subject must be distinct fields linking to locally owned entities.
  Core follows existing owner chains to match the grantee to the caller and
  resolve the **subject's Core identity**. Shared access reaches that identity's
  rows across owned entities in the app, including sibling enrollments; it is
  not limited to descendants of the referenced subject row. Sharing never
  follows another sharing relation.
- Each target requires its own `app:{app}:{entity}.read.shared` permission, such
  as `app:support:note.read.shared`. Core generates these keys; do not redeclare
  `.shared` as a custom scope. Reading one target grants no access to another.
  Source/intermediate-table read grants are not required for resolution.
- Existing owner definitions and `.own` permissions remain separate. Shared
  reads grant no writes, owner changes, or assignment-management authority.
  Authorize assignment creation/update/deletion explicitly; never add automatic
  admin or user grants.
- **If an app declares any `share`, every owned entity in that app must have a
  nonsensitive owner field.** Core rejects `owner: true` combined with
  `sensitive: true`, including on intermediate owner links and owned entities
  not directly referenced by the assignment. Keep confidential data separate
  from ownership keys: sharing resolvers return those keys.
- `activeWhen.isNull` names a nullable date or timestamp field, not SQL.
  Only NULL is active; any non-NULL value, including a future date, deactivates
  the assignment. Under governed `READ COMMITTED`, a committed revocation is
  visible to the next statement, including inside a callback transaction.
  Other independent grants may still authorize a read.

Shared membership uses `column IN (SELECT rootcx_shared...)`, with a hashed
SubPlan evaluated once in the performance regression and indexed assignment
lookups. The table-wide OR `.own` OR `.shared` policy union may still scan the
target sequentially; do not promise an always-indexed or index-only target scan.

---

## Schema Sync

Core validates declarations and performs SQL admission before applying supported
schema changes. Governance owns the versioned row-access contract and sensitive
projection, reconciling them with resolvers, column privileges, and RLS
transactionally per app.

- Raw manifest `checks`, index expressions (`expr`), partial-index predicates
  (`where`), operator classes (`ops`), and index storage parameters (`with`) are
  refused. Use field enums and supported column-based indexes. Core-generated
  indexes, including active-assignment indexes, remain supported.
- Unexpected legacy routines, views, triggers, rules, or other unsupported SQL
  artifacts require an independently controlled administrator migration.
  Admission does not certify a historically compromised Core/global database.
- Pending `backend/migrations/*.sql` files cause deployment/start refusal:
  their SQL is neither executed nor silently marked applied. Archives with no
  pending files remain deployable subject to admission. Use the declarative
  manifest or an independently authorized administrator migration.
- Backend uploads never execute package lifecycle scripts, including root-package
  and trusted dependency scripts, even when no SQL files are pending. Dependency
  installation uses `--ignore-scripts` with the inherited Core environment
  cleared. Build required artifacts in a controlled build pipeline before upload
  and include their outputs; do not move privileged setup into `package.json`.
- `onStart` grants no implicit data authority. Lifecycle and anonymous workers
  use their fixed no-user identity through governed transactions for collections
  and SQL. Initialize data through an explicitly authorized operation.

### Manifest ↔ DB contract

`dataContract` fields map to columns. Auto-columns (`id UUID`, `created_at`, `updated_at`) added by Core — omit from manifest `fields`. Type mapping: `text`→`TEXT`, `number`→`DOUBLE PRECISION`, `decimal`→`NUMERIC` or `NUMERIC(precision,scale)`, `boolean`→`BOOLEAN`, `date`→`DATE`, `timestamp`→`TIMESTAMPTZ`, `json`→`JSONB`, `file`→`TEXT`, `entity_link`→`UUID`, `[text]`→`TEXT[]`, `[number]`→`DOUBLE PRECISION[]`.
