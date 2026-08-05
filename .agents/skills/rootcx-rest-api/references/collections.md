# Core REST API — Collections

Base: `/api/v1/apps/{app_id}/collections/{entity}`

| Method | Path | Body | Response |
|--------|------|------|----------|
| GET | `/` | — | `T[]` |
| POST | `/` | `{field:value,...}` | `T` (201) |
| POST | `/bulk` | `[{...},...]` | `T[]` (201) |
| GET/POST | `/imports` | import request | import run (202) |
| POST | `/query` | `QueryOptions` | `{data:T[],total:number}` |
| GET | `/{id}` | — | `T` |
| PATCH | `/{id}` | `{field:value,...}` | `T` |
| DELETE | `/{id}` | — | `{message:string}` |

**GET list — query params (flat, no bracket syntax):**
- Filter: field name directly as param → `?contact_id=uuid&status=active`
- `sort` — field name (must exist in entity or `created_at`/`updated_at`/`id`), default `created_at`
- `order` — `asc` or `desc`, default `desc`
- `limit` — 1–1000, no default (returns all if omitted)
- `offset` — integer ≥ 0

**POST /query — body (JSON):**
- `where` — nested filter object (see operators below)
- `orderBy` — field name, default `created_at`
- `order` — `asc`/`desc`, default `desc`
- `limit` — 1–1000, default 100
- `offset` — integer ≥ 0

**Where operators:** `$eq` `$ne` `$gt` `$gte` `$lt` `$lte` `$like` `$ilike` `$in` `$nin` `$contains` `$isNull`
**Logical:** `$and` `$or` (arrays) `$not` (object)
**Shorthand:** `{"field":"value"}` = `{"field":{"$eq":"value"}}`, `{"field":null}` = IS NULL

## Large collection imports

Use `/bulk` only for interactive JSON batches of at most 1,000 rows. For a large, already-normalized dataset, enqueue a worker job and stream rows through the governed import path:

```ts
await ctx.collection("catalog_offer").importRows(rows, {
  mode: "append",
  columns: ["import_run_id", "source_item_id", "description"],
  sourceFileId: fileId,
  idempotencyKey: `${checksum}:mapping-v1`,
});
```

The Core accepts `append`, `upsert`, and atomic `replace`. Existing collection permissions govern the operation (`create`; `create+update`; or `create+update+delete`). A linked Storage file also requires `storage.read`. XLSX/CSV parsing and business mapping stay in the app; Core owns the streaming CSV transport, temporary staging table, RLS-governed publication, retries, progress, and summary audit event.

REST lifecycle: `POST /imports`, `GET /imports`, `GET|DELETE /imports/{id}`, and `POST /imports/{id}/retry`. POST normalized CSV to the one-hour, single-use `upload_url` returned by create/retry.

Empty streams are rejected by default, especially to prevent an accidental empty `replace`. Set `allowEmpty: true` only for an intentionally empty publication.
