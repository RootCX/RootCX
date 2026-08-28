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
- `"sensitive": true` keeps a column out of every API response, filter and sort. It stays writable and usable in SQL inside the app; it just never travels back over the wire.
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

  A role holding `app:school:submission.read.own` now sees the submissions hanging off its own enrollments, and no others. Ownership is resolved from the data alone, so grants on `assignment` or `enrollment` neither widen nor narrow it. Refused at install: a chain that loops, one spanning more than four entities, one ending on an entity that declares no owner, and an `entity_link` owner with no `references`. Cost: one indexed lookup per link, so keep the link columns indexed (`entity_link` already is).

---

## Schema Sync

On install/deploy, Core runs `CREATE SCHEMA IF NOT EXISTS` + `CREATE TABLE IF NOT EXISTS` for each entity in `dataContract`. Then `sync_schema` diffs DB vs manifest and auto-applies all changes (add/drop columns, alter types, nullability, defaults, check constraints). Studio shows a confirmation dialog before applying.

### Manifest ↔ DB contract

`dataContract` fields map to columns. Auto-columns (`id UUID`, `created_at`, `updated_at`) added by Core — omit from manifest `fields`. Type mapping: `text`→`TEXT`, `number`→`DOUBLE PRECISION`, `decimal`→`NUMERIC` or `NUMERIC(precision,scale)`, `boolean`→`BOOLEAN`, `date`→`DATE`, `timestamp`→`TIMESTAMPTZ`, `json`→`JSONB`, `file`→`TEXT`, `entity_link`→`UUID`, `[text]`→`TEXT[]`, `[number]`→`DOUBLE PRECISION[]`.
