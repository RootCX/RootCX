# Public data

A publication exposes an approved set of rows and fields through declared public
RPCs or collection reads. Install this manifest, deploy the worker, then approve
the publication:

```json
{
  "appId": "catalog",
  "name": "Catalog",
  "version": "1.0.0",
  "dataContract": [{
    "entityName": "products",
    "fields": [
      {"name": "name", "type": "text"},
      {"name": "published", "type": "boolean"},
      {"name": "internal_notes", "type": "text"}
    ]
  }],
  "public": {
    "publications": [{
      "name": "catalog",
      "entity": "products",
      "actions": ["list", "read"],
      "fields": ["name"],
      "where": {"published": true}
    }],
    "rpcs": [{"name": "list", "publications": ["catalog"]}],
    "collections": [{
      "entity": "products",
      "actions": ["list", "read"],
      "publication": "catalog"
    }]
  }
}
```

```ts
serve({
  rpc: {
    list: async (_params, _caller, ctx) =>
      ctx.collection("products").findPage({
        limit: 20, orderBy: "name", order: "asc"
      })
  }
});
```

Core applies the fixed `where` and field projection to `find`, `findPage`, and
`findOne`. Caller filters can narrow the result using published fields; they
cannot replace the fixed filter or filter/sort by hidden fields. Page totals
count only visible rows. Only explicitly listed fields are returned.
Without `orderBy`, public reads sort by the first approved output field
alphabetically. Set `orderBy` explicitly for predictable catalog ordering.
Unpaginated `find` rejects results over 10,000 rows; public read responses are
limited to 4 MiB. Use `findPage` and reduce its page size for larger results.

## Approve and call

Using a provider approver's bearer token, first retrieve the current declarations:

```sh
curl -sS "$CORE_URL/api/v1/apps/catalog/publications" \
  -H "Authorization: Bearer $TOKEN"
```

Copy `consumerInstallationId` and `providerInstallationId` from the desired
declaration in that array:

```sh
curl -sS -X POST "$CORE_URL/api/v1/apps/catalog/publications/catalog/approve" \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{
    "consumerInstallationId": "<current consumer UUID>",
    "providerInstallationId": "<current provider UUID>",
    "reason": "Publish the reviewed product catalog"
  }'
```

Approval requires `app:catalog:publications.approve` or
`admin:publications.manage`. Optionally supply `expiresAt` as a future RFC 3339
timestamp. Approval returns `201`; stale installation IDs return `409`. After a
contract change rotates an installation, retrieve fresh IDs and approve again.

The following calls need no token:

```sh
curl -sS "$CORE_URL/api/v1/apps/catalog/rpc" \
  -H 'Content-Type: application/json' \
  -d '{"method":"list","params":{}}'

curl -sS --get "$CORE_URL/api/v1/public/apps/catalog/collections/products" \
  --data-urlencode 'where={"name":"Example"}' \
  --data-urlencode 'limit=20' --data-urlencode 'offset=0' \
  --data-urlencode 'orderBy=name' --data-urlencode 'order=asc'

curl -sS "$CORE_URL/api/v1/public/apps/catalog/collections/products/$PRODUCT_ID"
```

The collection list returns `{"data":[{"name":"Example"}],"total":1}`; a record
read returns the projected object, or `404` when it is outside the publication.
`$PRODUCT_ID` is the record's UUID, even when `id` is absent from the approved
output fields. Worker code can likewise use `findOne({id: productId})`.
Adding a valid user JWT does not broaden a public RPC's authority.

## Remote data and ownership

For another app's data, add `"app": "provider"` to the publication and read through
the remote collection API:

```ts
serve({
  rpc: {
    list: async (_params, _caller, ctx) =>
      ctx.remote("provider").collection<{name: string}>("products").findPage({
        limit: 20, orderBy: "name", order: "asc"
      })
  }
});
```

The provider must approve with `app:provider:publications.approve` or
`admin:publications.manage`. An active cross-app grant from consumer to provider
is also required; the effective actions and fields are the intersection of the
grant and publication. The grant must also authorize fields used by the fixed
publication predicate: this example needs `published` in the grant's `fields`
alongside `name`, even though `published` is omitted from public output.

Direct public collection routes support remote publications too. Keep the
`public.collections` declaration pointing to the remote publication and call the
same consumer route, `/api/v1/public/apps/catalog/collections/products` (or its
`/{id}` record route). Core resolves the provider from the publication.

`releaseOwnership` defaults to `false`: public execution does not borrow the
visitor's ownership or the approver's identity. For an owned collection, set
`"releaseOwnership": true` only when the provider intends to disclose the rows
matching the fixed filter regardless of owner.

## Pause, resume, withdraw, and inspect

Public execution permits declared collection reads, not writes, raw SQL, or job
enqueueing. An approver can temporarily disable a publication and explicitly
enable it again. Both operations require a reason; enable rechecks the approved
contract, installation IDs, and expiry.

```sh
PUBLICATION_URL="$CORE_URL/api/v1/apps/catalog/publications/catalog"

curl -sS -X POST "$PUBLICATION_URL/disable" \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"reason":"Pause catalog for review"}'

curl -sS -X POST "$PUBLICATION_URL/enable" \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"reason":"Review complete"}'

curl -sS -X POST "$PUBLICATION_URL/revoke" \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"reason":"Withdraw catalog"}'

curl -sS "$PUBLICATION_URL/audit" -H "Authorization: Bearer $TOKEN"
```

Audit returns approval and lifecycle events with actor, reason, snapshot, and
timestamp. Revocation is terminal for that approval; publishing again requires
a fresh approval. Disablement, revocation, and expiry are checked again on use,
including existing workers. Revoking a remote grant independently removes remote
access; enabling a publication does not restore that grant.
