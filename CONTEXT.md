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
| SQL admission | Catalog and declaration checks that refuse unsupported app SQL artifacts; not certification of a historically compromised global database |
| Lifecycle worker | The fixed no-user worker that runs `onStart` without implicit data authority |

The [row-access guide](docs/row-access.md) defines usage.
[ADR 0006](docs/adr/0006-governed-row-access.md) records governed row access;
[ADR 0007](docs/adr/0007-resource-sharing.md) adds explicit resource sharing.
