---
name: rootcx-manifest
description: Writing or editing manifest.json for a RootCX app — defining the data contract, entities, field types, entity links, RBAC permissions, and understanding how Core syncs the schema to PostgreSQL on install/deploy.
version: 0.1.0
---

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

`text` `number` `boolean` `date` `timestamp` `json` `file` `entity_link` `[text]` `[number]`

### Rules

- `id`, `created_at`, `updated_at` are auto-generated — omit from `fields`
- `entity_link` requires `"references": { "entity": "<target>", "field": "id" }`. `<target>` is `"<entity>"` (same app) or `"core:users"` (FK → `rootcx_system.users`, `ON DELETE SET NULL`). Cross-app refs not yet supported.
- `"required": true` = mandatory on create; omit key for optional
- `"enum_values": [...]` restricts text fields to fixed values

---

## Schema Sync

On install/deploy, Core runs `CREATE SCHEMA IF NOT EXISTS` + `CREATE TABLE IF NOT EXISTS` for each entity in `dataContract`. Then `sync_schema` diffs DB vs manifest and auto-applies all changes (add/drop columns, alter types, nullability, defaults, check constraints). Studio shows a confirmation dialog before applying.

### Manifest ↔ DB contract

`dataContract` fields map to columns. Auto-columns (`id UUID`, `created_at`, `updated_at`) added by Core — omit from manifest `fields`. Type mapping: `text`→`TEXT`, `number`→`DOUBLE PRECISION`, `boolean`→`BOOLEAN`, `date`→`DATE`, `timestamp`→`TIMESTAMPTZ`, `json`→`JSONB`, `file`→`TEXT`, `entity_link`→`UUID`, `[text]`→`TEXT[]`, `[number]`→`DOUBLE PRECISION[]`.
