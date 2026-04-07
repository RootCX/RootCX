---
name: rootcx-rest-api
description: Calling the RootCX Core REST API directly — collections CRUD, bulk/query endpoints, integrations bind and actions, async jobs queue. Covers where operators, query syntax, and auth. Use when writing worker code, external scripts, or tests that hit the Core HTTP API.
version: 0.1.0
---

# RootCX Core REST API

Base: `{runtime_url}/api/v1`. Authenticated requests use `Authorization: Bearer {authToken}`.

Load `references/collections.md`, `references/integrations.md`, or `references/jobs.md` for the full endpoint tables.

## Collections (summary)

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

## Where operators

**Operators:** `$eq` `$ne` `$gt` `$gte` `$lt` `$lte` `$like` `$ilike` `$in` `$nin` `$contains` `$isNull`
**Logical:** `$and` `$or` (arrays) `$not` (object)
**Shorthand:** `{"field":"value"}` = `{"field":{"$eq":"value"}}`, `{"field":null}` = IS NULL

## Integrations (summary)

- Bind: `/api/v1/apps/{app_id}/integrations` (list/bind/update/unbind)
- Actions + auth: `/api/v1/integrations/{integration_id}` (actions, auth start/credentials/disconnect)

**From a worker:** `POST {runtime_url}/api/v1/integrations/{integration_id}/actions/{action_id}` with `Authorization: Bearer {authToken}`, body = action input.

## Jobs (summary)

Async job queue managed by Core. Workers enqueue jobs via REST, Core dispatches them back to the worker via IPC.

Base: `/api/v1/apps/{app_id}/jobs` — POST `/`, GET `/`, GET `/{job_id}`.

**Job statuses:** `pending` → `running` → `completed` | `failed`

Use jobs for long-running work (bulk fetches, batch imports, async syncs) that would exceed the 30s RPC timeout.
