# Collection imports

Collection imports are the governed data path for loading large, already-normalized datasets into a manifest collection. They complement `POST /bulk`: `/bulk` is an interactive JSON operation capped at 1,000 records, while an import is a durable run backed by PostgreSQL `COPY`.

The Core does not parse XLSX or invent source mappings. A worker reads the source file from RootCX Storage, validates and normalizes it, then streams typed row objects with:

```ts
await ctx.collection("catalog_offer").importRows(rows, {
  mode: "append",
  columns: ["import_run_id", "source_item_id", "description", "price"],
  sourceFileId: fileId,
  idempotencyKey: `${checksum}:mapping-v1`,
});
```

`rows` may be an `Iterable` or `AsyncIterable`. The worker prelude produces CSV incrementally with backpressure; it never buffers the complete dataset. `null` and `undefined` become PostgreSQL `NULL`, dates become ISO strings, and objects/arrays become JSON.

## Modes and governance

| Mode | Publication behavior | Required collection permissions |
|---|---|---|
| `append` | Insert every staged row | `create` |
| `upsert` | Insert or update on `conflictColumns` | `create`, `update` |
| `replace` | Delete the collection then insert the staged rows in one transaction | `create`, `update`, `delete` |

`upsert.conflictColumns` must exactly match a valid, non-partial unique index. When `sourceFileId` is present, `app:{appId}:storage.read` is also required. The Core checks the actor, delegated authority, source checksum, and all permissions when the run is created and again immediately before publication. Publication executes as `rootcx_app_executor` under the normal RLS policies.

No importer role or database credential is introduced. A worker receives only a one-hour, single-use upload URL. The database stores only its hash.

## Runtime behavior

Rows are copied into a connection-local PostgreSQL temporary table through a dedicated two-connection pool. One import runs at a time per tenant, and only one active import may target a given collection. The temporary table is automatically discarded if the connection or Core dies.

Publication is atomic. A failed validation, constraint, permission, or publication leaves the previously published collection unchanged. Row-level audit snapshots are suppressed during publication and replaced by one `BULK_IMPORT` audit event containing the mode, row count, byte count, actor, and import id.

Interrupted runs become `failed` after a Core restart and can be retried from the retained source file. The default stream limit is 16 GiB and can be changed with `ROOTCX_IMPORT_MAX_BYTES`.

## REST API

Base: `/api/v1/apps/{appId}/collections/{entity}/imports`

| Method | Path | Purpose |
|---|---|---|
| `POST` | `/` | Create a pending import and receive `upload_url` |
| `GET` | `/` | List the latest 100 visible runs |
| `GET` | `/{id}` | Read one run; creators may always read their own run |
| `DELETE` | `/{id}` | Cancel a pending or loading run |
| `POST` | `/{id}/retry` | Re-arm a failed or cancelled run with a new upload URL |

Create body:

```json
{
  "mode": "upsert",
  "columns": ["supplier_ref", "description", "price"],
  "conflictColumns": ["supplier_ref"],
  "allowEmpty": false,
  "sourceFileId": "4ea2a49f-3195-4a42-a026-dc85abf508d0",
  "idempotencyKey": "sha256:mapping-v2"
}
```

POST the normalized CSV stream directly to the returned `upload_url`. The URL is intentionally unauthenticated because it is an unguessable, expiring, single-use capability scoped to exactly one pre-authorized import.

An empty stream is rejected by default so an accidental empty `replace` cannot erase a collection. Set `allowEmpty: true` only when publishing an intentionally empty dataset.
