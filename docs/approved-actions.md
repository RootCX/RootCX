# Approved backend actions

Available in Core v0.27.0. An
administrator approves an action's exact local data access and backend release.
Ordinary users receive permission to call that action, without receiving its
collection permissions. The reviewed backend decides which rows and fields to
return and which business operations to permit.

## Runtime and trust boundary

Approved actions use Core's existing cross-platform Bun process architecture
and the same deployment prerequisites as ordinary workers. Each invocation
starts a fresh approved process with Core-bound caller, action and approval
context. Core stops it after the invocation; it is not reused in the ordinary
worker cache. The feature adds no OS-identity or volume-ownership prerequisite.
Managed app data, user configuration and keys remain in place.

The approval trusts the **entire deployed backend artifact**, including resolved
dependencies, module initialization, and every handler sharing that JavaScript
runtime. A function boundary is not an isolation boundary: code in that process
can use the action's data authority. Review the complete manifest and installed
backend tree, not just the selected handler or an uploaded source archive.
Core hashes the installed tree after dependency installation and captures a
versioned snapshot containing the resolved dependencies. It verifies the hash
at approval and again at approved-process startup, and executes from that
snapshot. External or cyclic artifact links are refused. Read-only file
settings are a defensive measure, not immutability against hostile processes
running under the same OS account; hash checks do not prevent interference
after verification.

Core enforces the declared local tables, CRUD verbs and readable columns through
a per-approval PostgreSQL role and generated RLS. Denied access raises database
permission/RLS errors, rather than relying on business filters to hide rows.
The read-lock support described below adds no mutation authority.
Caller permissions do not add more data authority.
This is entity-wide access, not a per-reader row policy; the backend must apply
the business assignment checks and output projection.
Sensitive column reads remain forbidden, including filters, expressions and
`RETURNING`; use explicit nonsensitive projections. Jobs, remote collections,
self-action/sub-action calls, tools, integrations, storage and event capabilities
are unavailable in this context, and credentials are not injected. The supported
data interface is local `ctx.collection`, `ctx.sql`, and `ctx.transaction`.
Ordinary apps remain untrusted at the governed IPC/SQL layer. Approved code is
trusted for business checks within its data ceiling. Host filesystem and
process confinement are outside the existing deployment model; a fresh process
does not establish complete malicious-app confinement. Host integrity,
dependency trust and output safety remain the operator's responsibility.

Foreign-key effects are part of that ceiling. Before approval, Core checks the
full chain of referential writes: deleting a parent with `ON DELETE CASCADE`
requires explicit child `delete`; `SET NULL`/`SET DEFAULT` require child
`update`; update cascades require child `update`. Every affected local entity
and verb must be requested and reviewed, or approval is rejected. Cross-app
referential writes cannot be approved. Core never silently expands the grant
to accommodate a cascade.

## A case shared by two readers

Create a directory containing the following `manifest.json` and
`backend/index.js`. No npm dependencies are needed.
Use lowercase snake_case action IDs: manifest permission-key validation rejects
uppercase letters. The IDs must match the RPC handler names, role grants and
CLI arguments. API/body properties such as `userId`, `caseId`, `backendDigest`
and `installationId` retain their camelCase spelling.

```json
{
  "appId": "case_actions",
  "name": "Assigned cases",
  "version": "1.0.0",
  "dataContract": [
    {
      "entityName": "case_file",
      "fields": [
        { "name": "title", "type": "text", "required": true },
        { "name": "private_notes", "type": "text" }
      ]
    },
    {
      "entityName": "assignment",
      "fields": [
        { "name": "case_id", "type": "entity_link", "required": true,
          "references": { "entity": "case_file", "field": "id" } },
        { "name": "user_id", "type": "entity_link", "required": true,
          "references": { "entity": "core:users", "field": "id" } },
        { "name": "revoked_at", "type": "timestamp" }
      ]
    },
    {
      "entityName": "interaction",
      "fields": [
        { "name": "case_id", "type": "entity_link", "required": true,
          "references": { "entity": "case_file", "field": "id" } },
        { "name": "author_id", "type": "entity_link", "required": true,
          "references": { "entity": "core:users", "field": "id" } },
        { "name": "message", "type": "text", "required": true }
      ]
    }
  ],
  "actions": [
    {
      "id": "assigned_cases",
      "name": "Read assigned cases",
      "authority": {
        "data": {
          "case_file": ["read"],
          "assignment": ["read"],
          "interaction": ["read"]
        }
      }
    },
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
  ]
}
```

These are ordinary business assignments, not a `share` declaration. There is
one case and one set of interactions, with a relation per reader; no business
records or per-reader result copies are created. `private_notes` is deliberately
an ordinary column here: its omission is a reviewed-code guarantee. Mark a
column `sensitive: true` when even approved code must not read it.

```javascript
const objects = ({ columns, rows }) =>
  rows.map(row => Object.fromEntries(columns.map((key, i) => [key, row[i]])));

serve({
  rpc: {
    assigned_cases: async (_params, caller, ctx) => {
      if (!caller?.userId) throw new Error("Sign in required");
      const cases = await ctx.sql(
        `SELECT c.id::text, c.title FROM case_file c
         WHERE EXISTS (
           SELECT 1 FROM assignment a
           WHERE a.case_id = c.id AND a.user_id = $1 AND a.revoked_at IS NULL
         ) ORDER BY c.id LIMIT 100`, [caller.userId]);
      const interactions = await ctx.sql(
        `SELECT i.id::text, i.case_id::text, i.author_id::text, i.message
         FROM interaction i WHERE EXISTS (
           SELECT 1 FROM assignment a
           WHERE a.case_id = i.case_id AND a.user_id = $1 AND a.revoked_at IS NULL
         ) ORDER BY i.created_at DESC, i.id LIMIT 100`, [caller.userId]);
      return { cases: objects(cases), interactions: objects(interactions) };
    },
    add_interaction: async (params, caller, ctx) => {
      if (!caller?.userId) throw new Error("Sign in required");
      if (typeof params?.caseId !== "string" ||
          !/^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i.test(params.caseId))
        throw new Error("Valid caseId required");
      if (typeof params.message !== "string" ||
          !params.message.trim() || params.message.length > 2000)
        throw new Error("Message must contain 1–2000 characters");
      return ctx.transaction(async tx => {
        const active = await tx.sql(
          `SELECT id FROM assignment
           WHERE case_id = $1 AND user_id = $2 AND revoked_at IS NULL
           ORDER BY id LIMIT 1 FOR SHARE`, [params.caseId, caller.userId]);
        if (!active.rows.length) throw new Error("Case is not assigned to you");
        const result = await tx.sql(
          `INSERT INTO interaction (case_id, author_id, message)
           VALUES ($1, $2, $3)
           RETURNING id::text, case_id::text, author_id::text, message`,
          [params.caseId, caller.userId, params.message.trim()]);
        return objects(result)[0];
      });
    }
  }
});
```

Core supplies `serve`, the app-scoped SQL search path and `caller.userId`.
Never accept the acting user ID from parameters. Both read statements check the
live assignment independently; they are not a single consistent snapshot.
No startup hook seeds data: the operator creates the example rows below.
The sample limits each result to 100 rows; add deliberate pagination for larger
workloads.

The create action locks an active assignment **inside the same transaction**
as the insert. Assignment `["read"]` is sufficient for `SELECT ... FOR SHARE`;
do not request `update` just to lock a row. Core supplies the technical
`UPDATE(primary_key)` ACL and an RLS `UPDATE USING` condition accepting approved
read access, while `WITH CHECK` remains false without declared `update`.
The lock succeeds, but changing even the primary key is denied.
`interaction.read` permits the explicit `RETURNING` projection. A callback or
statement error rolls back the create, even when application code catches a
statement error.

## Deploy, review and approve

Run from the app directory with `rootcx` connected to an administrator account.
The commands below require a CLI build containing `apps actions`; a Core-only
release does not update installed CLI binaries. The HTTP approval API described
below is available independently. Deployment never approves an action.

```sh
rootcx deploy
rootcx apps actions list case_actions
rootcx apps actions list case_actions --json > review.json
```

Review the full manifest, the installed backend and resolved dependencies on
the Core host, and the exact authority shown for each chosen action. The JSON
snapshot contains `revision`, `backendDigest`, `installationId`, and
`actions[{id,authority,approvalId,status}]`. Missing revision/digest means there
is no complete release to approve. Inspection requires `admin:apps.deploy`;
approval and revocation require an administrator.

Interactive approval displays the selected action's access and all three
identifiers, then submits that same snapshot after confirmation:

```sh
rootcx apps actions approve case_actions assigned_cases
rootcx apps actions approve case_actions add_interaction
```

For automation, use identifiers from an **already reviewed** saved snapshot
(`jq` is required below). Do not fetch fresh values and blindly approve them.

```sh
rootcx apps actions approve case_actions assigned_cases --yes \
  --revision "$(jq -er '.revision' review.json)" \
  --digest "$(jq -er '.backendDigest' review.json)" \
  --installation "$(jq -er '.installationId' review.json)"
```

The API is `GET /api/v1/apps/{app}/action-approvals`,
`POST /api/v1/apps/{app}/action-approvals/{action}` with
`{revision,backendDigest,installationId}`, and `DELETE` on that action URL.
The POST must match all three reviewed values; a stale review returns HTTP 409 and
must be reviewed again. No `manifestDigest` field is needed or accepted.

The release revision rotates on **every backend deployment**, even with
identical bytes, and on **every full manifest change**, including cosmetic
name/version/description changes. Installation identity separately prevents
reuse across installation generations. Core also stores and checks the full
approved manifest. Thus revision + installation + backend digest pin this
review; restoring old code or an old manifest never revives an old approval.
A failed backend replacement can leave the digest absent and approvals revoked.

## Grant only the actions and exercise the flow

Use two existing ordinary Core users with no inherited app wildcard, entity
CRUD, or administrator permissions. Set `CORE_URL`, `ADMIN_TOKEN`,
`READER_A_ID`, `READER_B_ID`, `READER_A_TOKEN`, and `READER_B_TOKEN` in your shell.
The CLI remains signed in as the administrator for data setup.

```sh
curl --fail-with-body -sS "$CORE_URL/api/v1/roles" \
  -H "Authorization: Bearer $ADMIN_TOKEN" -H 'Content-Type: application/json' \
  --data '{"name":"case_reader","inherits":[],"permissions":[
    "app:case_actions:action:assigned_cases",
    "app:case_actions:action:add_interaction"
  ]}'
for reader in "$READER_A_ID" "$READER_B_ID"; do
  curl --fail-with-body -sS "$CORE_URL/api/v1/roles/assign" \
    -H "Authorization: Bearer $ADMIN_TOKEN" -H 'Content-Type: application/json' \
    --data "$(jq -n --arg user "$reader" '{userId:$user,role:"case_reader"}')"
done

CASE_ID=$(rootcx data create case_actions case_file \
  --body '{"title":"Shared case","private_notes":"Operator-only note"}' | jq -er '.id')
ASSIGNMENT_A=$(rootcx data create case_actions assignment --body \
  "$(jq -n --arg c "$CASE_ID" --arg u "$READER_A_ID" '{case_id:$c,user_id:$u}')" | jq -er '.id')
rootcx data create case_actions assignment --body \
  "$(jq -n --arg c "$CASE_ID" --arg u "$READER_B_ID" '{case_id:$c,user_id:$u}')"
rootcx data create case_actions case_file --body '{"title":"Unassigned case"}'

for reader_token in "$READER_A_TOKEN" "$READER_B_TOKEN"; do
  curl --fail-with-body -sS "$CORE_URL/api/v1/apps/case_actions/rpc" \
    -H "Authorization: Bearer $reader_token" -H 'Content-Type: application/json' \
    --data '{"method":"assigned_cases","params":{}}'
done

curl --fail-with-body -sS "$CORE_URL/api/v1/apps/case_actions/rpc" \
  -H "Authorization: Bearer $READER_A_TOKEN" -H 'Content-Type: application/json' \
  --data "$(jq -n --arg c "$CASE_ID" \
    '{method:"add_interaction",params:{caseId:$c,message:"First contact"}}')"
```

Both readers see the same assigned case ID, no unassigned case and no private
notes. Calling `assigned_cases` again as reader B also returns the interaction
created by A. A frontend uses
`client.rpc("case_actions", "add_interaction", { caseId, message })` with its own
authenticated runtime client.

Direct collection access does not inherit the action's authority:

```sh
curl -sS -w '\nHTTP %{http_code}\n' \
  "$CORE_URL/api/v1/apps/case_actions/collections/case_file" \
  -H "Authorization: Bearer $READER_A_TOKEN"
curl -sS -w '\nHTTP %{http_code}\n' \
  "$CORE_URL/api/v1/apps/case_actions/collections/interaction" \
  -H "Authorization: Bearer $READER_A_TOKEN" -H 'Content-Type: application/json' \
  --data "$(jq -n --arg c "$CASE_ID" --arg u "$READER_A_ID" \
    '{case_id:$c,author_id:$u,message:"Bypass attempt"}')"
```

The direct GET returns HTTP 200 with `[]`: the caller has no readable rows.
The direct POST is rejected with HTTP 403. A successful empty list does not
mean that the caller received the action's data permissions.
An approved action requires its fine
`app:{app}:action:{id}` permission plus a current approval; plain `invoke`
is insufficient. Invoking an authority action without a current approval returns
HTTP 403. For an ordinary declared action without `authority`, the
normal RPC rule is `invoke` **OR** the fine action permission. Undeclared RPCs
require `invoke`. Declaring authority does not grant any user permissions.

## Two different revocations

Business assignment revocation belongs to the app's transaction rules. As the
administrator, revoke A's assignment by updating the same row the create action
locks:

```sh
rootcx data update case_actions assignment "$ASSIGNMENT_A" --body \
  "$(jq -n --arg now "$(date -u +%Y-%m-%dT%H:%M:%SZ)" '{revoked_at:$now}')"
```

If creation acquires the shared row lock first, this update waits for that
create to commit or roll back. If revocation commits first, the locking
assignment query finds no active row and creation is refused. Every path that
revokes or reassigns access must update/delete that same assignment row under
normal transactional locking. If multiple active assignments exist, access
lasts until all applicable assignments are revoked. Earlier returned data
cannot be recalled. B's separate assignment and the shared case data remain.

Operator approval revocation removes the execution authority for everyone:

```sh
rootcx apps actions revoke case_actions add_interaction
rootcx apps actions list case_actions
```

It waits for already admitted action database transactions to finish before
returning. Those transactions may commit before revocation completes; a later
transaction cannot use the revoked approval, even from a previously started
invocation. This barrier does not implement assignment policy or recall
previous results. `assigned_cases` remains independently approved. Reapproval
requires another explicit review.

For a deployment check, verify that both readers can use their fine grants,
an unassigned case cannot be mutated, a reader without `add_interaction` is
refused, A loses access after assignment revocation while B retains it, and
both readers lose creation after approval revocation. Also verify that a
cosmetic manifest change or identical backend redeployment makes previous
approvals pending and stale review identifiers fail approval.

See [ADR 0008](adr/0008-approved-actions.md) for the architectural decision and
[governed row access](row-access.md) for ordinary ownership/sharing semantics.
