# Resource sharing: from setup to individual revocation

This walkthrough uses the complete `projects` manifest in
[row-access.md](row-access.md#share-an-exact-resource), on a Core build that
supports `scope: "resource"`. Deploy that app through the normal `rootcx deploy`
workflow before creating roles or data.

The outcome is one project and its documents shared with two readers. Revoking
one reader's assignment leaves the other reader's access intact. There is one
copy of each document, regardless of the number of readers.

## 1. Identify the people and their responsibilities

Use four authenticated Core accounts:

| Account | Responsibility |
| --- | --- |
| Instance administrator | Install the app, provision roles and create the initial records |
| Sharing manager | Create, end or delete assignments |
| Reader A | Read the explicitly shared project and documents |
| Reader B | Read the same shared project and documents independently |

Each account uses its own Bearer access token. All paths below are relative to
the same Core instance. JSON writes use `Content-Type: application/json`.

With each account's token, call `GET /api/v1/auth/me`; the response's `id` is its
Core user UUID. Call the manager's UUID `UM`, and the readers' UUIDs `U1` and `U2`.
These are placeholders for real returned UUIDs, not values to insert literally.
Use existing accounts authenticated through the instance's configured sign-in
flow.

## 2. Provision the two roles as the administrator

Create a reader role:

```http
POST /api/v1/roles
Authorization: Bearer <administrator-token>
Content-Type: application/json

{
  "name": "projects_reader",
  "permissions": [
    "app:projects:project.read.shared",
    "app:projects:document.read.shared"
  ]
}
```

Both reader accounts receive this same role. For a documents-only view, omit
`project.read.shared`: document resolution does not require permission to read
the project, folders, members or assignments.

Create a manager role:

```http
POST /api/v1/roles
Authorization: Bearer <administrator-token>
Content-Type: application/json

{
  "name": "projects_sharing_manager",
  "permissions": [
    "app:projects:assignment.read",
    "app:projects:assignment.create",
    "app:projects:assignment.update",
    "app:projects:assignment.delete"
  ]
}
```

This manager can manage **all assignments in this app**. The administrator
supplies the member/project IDs below, so the manager needs no other reads in
this walkthrough. A UI that lets the manager browse members or projects needs
separately authorized read permissions on those entities. Sharing does not
implicitly grant management authority or a narrower per-project management scope.

As the administrator, call `POST /api/v1/roles/assign` for these three payloads,
replacing the UUID placeholders:

```json
{"userId": "UM", "role": "projects_sharing_manager"}
```

```json
{"userId": "U1", "role": "projects_reader"}
```

```json
{"userId": "U2", "role": "projects_reader"}
```

Role creation and assignment require the instance administrator; possessing an
app's data permissions is insufficient. Successful calls return HTTP 200.

Inspect `GET /api/v1/permissions` with each participant's own token. Permissions
are additive: an existing administrator role, wildcard or ordinary
`project.read`/`document.read` grant would bypass the intended sharing-only view.
Use dedicated restricted accounts for this exercise. To remove an unintended
role, the administrator can inspect `GET /api/v1/roles/assignments`, then call
`POST /api/v1/roles/revoke` with that account's `userId` and `role`. Do not change
an existing role used by other people merely to configure these readers.

## 3. Create the members and original data as the administrator

Send each payload to `POST /api/v1/apps/projects/collections/<entity>`. Each
successful creation returns HTTP 201 and a record with an `id`. Save that ID
under the label in the table, replacing labels in subsequent payloads with
their actual UUIDs. Core creates IDs and timestamps; do not supply them.

| Saved ID | Entity | Payload |
| --- | --- | --- |
| M1 | `member` | `{"user_id":"U1"}` |
| M2 | `member` | `{"user_id":"U2"}` |
| P1 | `project` | `{"name":"Shared project"}` |
| P2 | `project` | `{"name":"Private project"}` |
| F1 | `folder` | `{"project_id":"P1"}` |
| F2 | `folder` | `{"project_id":"P2"}` |
| D1 | `document` | `{"folder_id":"F1","body":"Shared document"}` |
| D2 | `document` | `{"folder_id":"F2","body":"Private document"}` |

`M1` and `M2` are local member record IDs; `U1` and `U2` are Core user IDs.
The assignment's `member_id` must receive **M1 or M2**, never U1 or U2.
Neither project has an Owner or Core identity.

At this point, each reader's `GET /api/v1/apps/projects/collections/document`
returns an empty array: a role grants the ability to read shared documents,
but no active assignment has shared any yet.

## 4. Share P1 with both readers as the manager

Use the manager's token for both requests:

```http
POST /api/v1/apps/projects/collections/assignment
Authorization: Bearer <manager-token>
Content-Type: application/json

{"member_id":"M1","project_id":"P1","end_date":null}
```

Save the returned ID as `A1`. Repeat with `member_id: M2` and save the ID as `A2`.
There are now two authorization relationships to the same project. No folder
or document is copied.

## 5. Verify reads as each reader

Use the respective reader's token, not the administrator's:

| Request | Expected result for both readers |
| --- | --- |
| `GET /api/v1/apps/projects/collections/project` | Only P1 |
| `GET /api/v1/apps/projects/collections/document` | Only D1 |
| `GET /api/v1/apps/projects/collections/document/D2` | HTTP 404; private document is not visible |

The collection list returns a JSON array of records. No application-side
authorization filter is needed: Core enforces the resource relationship.
The `via` path is read from the target toward the subject:
`document.folder_id → folder.project_id → project`.
Traversing a folder does not grant permission to list folders.

A worker invoked by that reader receives the same row restrictions:

```typescript
await ctx.sql("SELECT id, body FROM projects.document");
```

For that optional worker path, the reader also needs the appropriate invocation
permission: `app:projects:invoke` for an ordinary undeclared RPC, or the declared
action's permission. The HTTP collection reads above need no invoke permission.
Sensitive fields stay excluded or SQL-denied; resource sharing does not vary
field visibility by reader.

## 6. Revoke only reader A as the manager

```http
PATCH /api/v1/apps/projects/collections/assignment/A1
Authorization: Bearer <manager-token>
Content-Type: application/json

{"end_date":"2026-09-17"}
```

Replace `A1` with the real assignment UUID. HTTP 200 confirms the update.
Any non-NULL date makes the assignment inactive immediately, including a future
date. This field is not an automatic expiry schedule.

Repeat the reads from step 5:

- Reader A sees empty project/document lists; a direct D1 read returns 404.
- Reader B still sees P1 and D1 because A2 remains active.
- Neither reader sees P2 or D2.

Deleting A1 with `DELETE /api/v1/apps/projects/collections/assignment/A1` has
the same effect on this route of access. To restore an ended assignment, the
authorized manager can set `end_date` back to `null`. Do not revoke the shared
reader role to end a single resource assignment.

Core observes committed revocation at the next statement, including within a
worker callback transaction. Already-returned data cannot be recalled. A second
active assignment or an independent broader permission can still authorize a
read; inspect those if access persists.
