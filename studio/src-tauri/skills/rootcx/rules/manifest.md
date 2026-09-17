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
