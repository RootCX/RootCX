# Core REST API — Collections

Base: `/api/v1/apps/{app_id}/collections/{entity}`

| Method | Path | Body | Response |
|--------|------|------|----------|
| GET | `/` | — | `T[]` |
| POST | `/` | `{field:value,...}` | `T` (201) |
| POST | `/bulk` | `[{...},...]` | `T[]` (201) |
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
