# Domain glossary

| Term | Meaning |
| --- | --- |
| Core | The trusted runtime that authorizes app operations and reconciles their database contracts |
| App | An installed manifest and backend whose data and capabilities are governed by Core |
| Core identity | A Core user identity at the end of an ownership chain; distinct from an app row's primary key |
| Owner | The Core identity derived from an entity's `owner: true` field, directly or through local owner links |
| Own scope | The `.own` permission scope, confining an operation to the caller's owned rows |
| Assignment | A row with a bounded `share` declaration relating a grantee to a subject while its activation field is NULL |
| Grantee | The assignment endpoint whose resolved Core identity receives shared-read access |
| Subject | The assignment endpoint identifying the resource to share, or the Owner whose rows are shared in identity mode |
| Resource | The exact app row referenced by an Assignment's Subject in resource mode; it needs no Owner or Core identity |
| Resource path | An explicit, bounded chain of local primary-key entity links from a readable target to the Resource |
| Shared-read scope | A target entity's `.read.shared` permission; grants reads through active assignments without changing ownership or granting writes |
| Sensitive field | A column withheld from the worker executor's `SELECT` privileges and omitted from generated read projections |
| Row-access contract | The validated, versioned Core projection used to reconcile ownership, sharing, sensitive privileges, and RLS |
| SQL admission | Checks on new manifest SQL declarations before DDL; existing database objects remain operator-managed and are not audited at boot |
| Lifecycle worker | The fixed no-user worker that runs `onStart` without implicit data authority |
| Approved action | A declared backend action whose exact local CRUD authority and executable release an administrator has reviewed and approved |
| Action authority | The approved local entity/verb ceiling used by reviewed backend code; distinct from the user's permission to invoke the action |
| Approval database role | An exact PostgreSQL role per approval; undeclared tables/verbs error, sensitive reads stay forbidden, and referential write effects require explicit authority |
| Approved read lock | Read authority permits `SELECT ... FOR SHARE` through a technical primary-key UPDATE ACL and RLS USING check; RLS WITH CHECK still denies mutations without declared update authority |
| Backend release | A post-dependency backend digest and revision; the revision rotates on every deployment and every full manifest change, including cosmetic changes |
| Approved artifact | The versioned backend snapshot, including resolved dependencies and all handlers sharing its runtime, trusted for business checks; its hash is verified at approval and process startup |
| Approved process | A fresh Bun worker per invocation with Core-bound caller/action/approval context; stopped after use, with the usual worker deployment model |
| Approval revocation | A barrier that waits for admitted action database transactions and prevents subsequent use; business assignment revocation remains an app transaction rule |

The [row-access guide](docs/row-access.md) defines usage.
[ADR 0006](docs/adr/0006-governed-row-access.md) records governed row access;
[ADR 0007](docs/adr/0007-resource-sharing.md) adds explicit resource sharing.
[Approved actions](docs/approved-actions.md) and
[ADR 0008](docs/adr/0008-approved-actions.md) describe reviewed backend authority.
Ordinary declared RPCs accept `invoke` or the fine action grant; approved actions
require the fine grant and a current approval. Approved execution preserves the
usual cross-platform Bun process architecture. Ordinary apps remain untrusted
at governed IPC/SQL boundaries; the full approved artifact is trusted for
business checks. Read-only snapshots are defensive, not immutable against
hostile processes under the same OS account. Host filesystem/process confinement
is outside this deployment model; platform test coverage requires actual runs.
