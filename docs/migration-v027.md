# Migration guide: Core v0.27.0 — governed row access

This guide includes the post-0.27.0 correction: historical database objects and
stored check/index declarations no longer fail boot-time SQL admission. The
released 0.27.0 binary still performs that inspection; use a Core containing the
correction. New submissions remain subject to SQL declaration validation.
Stored non-access column types are not revalidated at boot; new submissions
still require supported field types.

The upgrade introduces assignment-based shared reads and enforces sensitive
columns in PostgreSQL. It also removes lifecycle collection bypass and
owner-executed app SQL migrations. Existing apps may need changes before they
can be admitted or started.

## Compatibility changes

| Existing behavior or artifact | Behavior with the post-0.27.0 correction |
| --- | --- |
| `owner: true` and `.own` permissions | Existing owner definitions and scoped operations remain intact |
| Assignment sharing | An explicit `share` declaration enables subject-identity reads, gated by each target's `.read.shared` permission |
| Exact resource sharing | Opt into `scope: "resource"` with explicit target paths; the subject needs no Owner and app data is not copied |
| Approved backend actions | Optional `actions[].authority.data` requests require operator approval of the exact release; callers need the action permission, not direct collection rights |
| Raw SQL reading a sensitive column | Denied by database column `SELECT` privileges, including expressions, filters, and `RETURNING` |
| `SELECT *` or `RETURNING *` on a table with sensitive columns | Denied; explicitly select safe columns |
| Generated reads of sensitive fields | Continue to omit those fields and reject filter/sort references |
| Writes to sensitive fields | Still governed by RLS; setting a value does not authorize reading it back |
| Collection access during `onStart` | Uses the fixed lifecycle identity with no implicit user or permissions |
| Pending backend `migrations/*.sql` files | Deployment/start refused; SQL is not executed or marked applied |
| No pending migration files | Deployment remains supported, subject to normal admission |
| Package lifecycle scripts during backend upload | Never executed, including root-package scripts and trusted dependency scripts; build required artifacts before upload |
| Raw manifest `checks`, expression/partial/opclass/index `with` specifications | Refused on new submissions, including reinstall; stored declarations are not executed during boot |
| Supported column-based indexes and Core-generated indexes | Remain supported |
| Existing SQL objects, including partial indexes | Remain operator-managed; no retroactive catalog audit at boot |

No upgrade step silently grants admin, user, shared-read, or assignment-write
permissions to preserve old behavior.

Approved actions introduce an explicit trust decision for reviewed backend code.
They use the usual cross-platform Bun worker spawning path, with a fresh
approved process per invocation and no new OS-identity or volume-ownership
prerequisite. Preserve existing managed app data, configuration and keys.
Review the full artifact, including dependencies and all handlers sharing its
runtime: that code is trusted for business checks. Core verifies versioned
snapshot hashes at approval and process startup. Read-only settings do not
make snapshots immutable against hostile processes under the same OS account.
Host filesystem/process confinement remains outside the existing deployment
model; ordinary apps remain untrusted at governed IPC/SQL boundaries.

See the [approved-action guide](approved-actions.md) for the complete
development/approval workflow. Exact database roles, permission/IPC checks,
transaction controls and the revocation barrier remain enforced. Backend
redeployment or any full manifest change, including cosmetic changes, retires
approvals. Ordinary actions retain caller-based governance.

## Operator procedure

### 1. Inventory before upgrading

Record each installed app's manifest, backend archive, migration files, and
legacy `schema_migrations` history. Distinguish files recorded as applied from
pending files; an existing ledger is history, not evidence that past SQL was
safe.

Inventory app-schema catalog definitions without invoking app routines or
selecting from app views. Include functions/procedures, views/materialized views,
triggers, rewrite rules, RLS policies, column types and defaults, generated
columns, constraints, indexes and their predicates/operator classes, inheritance,
partitions, and foreign tables. Keep definitions for review, rather than relying
on object names or comments that claim Core ownership.

Review backend queries for sensitive-column references, `SELECT *`,
`RETURNING *`, and `onStart` data initialization. Inspect manifests for raw
checks and unsupported index fragments. Inventory existing ownership
declarations and explicit roles before introducing shared-read permissions.

Inventory root-package and dependency lifecycle scripts, including dependencies
listed as trusted. Identify generated files, native artifacts, or other outputs
that the backend previously produced during dependency installation.

The six integrations bundled in this source tree—Gmail, Google Calendar,
IMAP/SMTP, Notion, Outlook, and Peppol—declare no `onStart` hook or SQL migration
files. Peppol writes collections from its webhook handler. This finding covers
the bundled sources, not separately downloaded, older, or customized backends.
Inventory deployed artifacts independently.

### 2. Back up and establish a trusted baseline

Take a consistent database backup and preserve the global role/privilege and
extension configuration, app artifacts, and migration history needed to restore
the installation. Verify restoration in an isolated environment before changing
production. Plan a maintenance window or otherwise stop relevant writes while
performing administrator migrations.

Historical app migrations ran with database-owner authority and could have
changed Core routines, roles, extensions, or objects outside the app schema.
Admission is not a forensic certification of those global objects. Establish
their provenance against a trusted baseline. If that cannot be done, restore or
rebuild a trusted database and separately review any data import; do not rely on
declaration admission to repair a compromised Core database.

### 3. Prepare manifests and backend code

Use field enums and supported declarative structures instead of raw `checks`.
Before submitting a manifest again, remove expression indexes (`expr`),
partial-index predicates (`where`), operator-class specifications (`ops`), and
index storage parameters (`with`). Simple supported column-based indexes remain
available. Do not reproduce Core's generated sharing indexes as app-supplied SQL.

Replace sensitive wildcard reads with explicit safe columns. Audit SQL
expressions, filters, sorting, and write `RETURNING` lists as well as SELECT
lists. Generated writes may return safe columns; writing a sensitive value
does not authorize reading it.

Move startup data initialization to an operation invoked by an explicitly
authorized principal, or to a reviewed administrator data migration. Do not
work around removal of lifecycle bypass by automatically assigning an admin or
user role to a worker.

Backend uploads never execute package lifecycle scripts, even when no SQL
migrations are pending. Dependency installation uses Bun's `--ignore-scripts`
with the inherited Core environment cleared. Root-package scripts and trusted
dependency scripts are both disabled; `trustedDependencies` is not an exception.
Build any required artifacts in a controlled build pipeline before upload and
include the required outputs in the deployment artifact. Do not move database
setup or privileged startup work into `package.json`: upload-time scripts must
not restore the Core authority removed from app SQL migrations and lifecycle
workers.

For assignments, use the minimal declaration in [row-access.md](row-access.md).
Keep owner definitions intact. Grant only the target `.read.shared` keys that
are required, and authorize management of assignment rows separately. Sharing
resolves the subject's Core identity: sibling enrollments and other owned
entities can be visible when their target permissions are granted.

That identity scope remains the default for compatibility. Use explicit
`scope: "resource"` when access must stop at one row and its declared target
paths. Resource subjects need no Owner or Core identity. Intermediate entities
do not become shared. Review writes to the path's relationship fields as well
as assignment management. This is a deliberate access-contract change; it does
not add field redaction or worker-only reads.

Sharing requires nonsensitive ownership fields. A shared resolver returns
ownership keys, so Core refuses `owner: true` combined with `sensitive: true`
in an app declaring sharing. Keep confidential values in separate fields.

### 4. Manage historical database artifacts independently

Existing partial indexes, constraints, routines, and other SQL objects no longer
require removal to pass boot. They remain under operator control. When an
artifact needs changing, use a reviewed administrator migration that preserves
application data and invariants. Removing a declaration from a manifest does not
by itself migrate the existing database object.

App deployments do not execute that migration for you. Prefer the declarative
manifest where it expresses the change. After independently completing and
verifying a required migration, remove the pending SQL files from the backend
archive. Do not insert ledger rows merely to suppress refusal. Pending files
are never silently marked applied, and historical files are never replayed.

Known Core policy names remain reserved reconciliation slots.

### 5. Upgrade and reconcile in staging first

Without an existing `rootcx_system.row_access_contracts` entry, Core validates
the stored manifest's structure and access declarations before migrating it into
the versioned contract. Identity-only and nonsharing contracts use version 1;
resource-sharing contracts use version 2. Core upgrades its metadata version
constraint without modifying app data. Later boots validate and
replay that contract through the same governance module.

Core owns both `row_access_contracts` and the `sensitive_fields` projection.
It reconciles those projections, resolvers, RLS, and column privileges
transactionally per app. Audit and hooks consume the sensitive projection;
operators and apps must not edit either projection as an authorization shortcut.
Unsupported contract versions and invalid access declarations fail closed.
Resolve the reported cause through the controlled migration path and retry.
This transaction does not make a whole fleet upgrade or all schema DDL atomic.

Pending SQL files cause backend upload to fail before dependency installation,
agent registration, or worker startup. Manual starts, lazy worker
starts, and restart recovery also refuse pending files left on disk. A trusted
legacy ledger may identify files as already applied; an archive containing only
those files, or no migration files, can still deploy and reports
`migrationsApplied: []`.

### 6. Verify access before restoring traffic

Use explicitly provisioned test principals to verify:

- Own reads and writes still follow the original ownership chains.
- Authorized `INSERT ... RETURNING` works for brand-new ownership values; do
  not replace the read-policy union with a precomputed allowed-key array.
- Shared reads expose the subject identity's intended rows, including sibling
  enrollments, only on targets with `.read.shared`; sharing does not recurse.
- Shared reads alone cannot create assignments, mutate another person's data,
  or reparent owned rows.
- A committed assignment revocation is observed by the next statement under
  `READ COMMITTED`, including inside a callback transaction. Any non-NULL
  `end_date`, even a future date, makes the assignment inactive.
- Generated reads omit sensitive fields; raw references, expressions, filters,
  and sensitive `RETURNING` fail; explicit safe projections and authorized writes
  still work.
- Lifecycle and anonymous workers cannot read private rows or fabricate
  assignments, and no roles were automatically granted.
- Deployments with no pending SQL files start; pending files produce an
  actionable refusal without SQL execution or ledger mutation.
- Backend uploads execute neither root-package nor trusted dependency lifecycle
  scripts, including uploads with no pending SQL files. Required prebuilt
  artifacts are present and the backend starts without install-time generation.

Include representative large-data query plans in staging verification. The
200,000-row regression showed that the table-wide OR `.own` OR `.shared` RLS
union can produce a target `Seq Scan` on an unfiltered SELECT. The final shared
predicate uses `column IN (SELECT rootcx_shared...)`, with a `hashed SubPlan`
built once in the regression and indexed assignment lookups. Check for repeated
resolver execution per target row, not for an unconditional target-index scan.
Use a trusted harness posing the restricted executor with RLS active; an owner
connection that bypasses RLS, forced index settings, or an added ownership WHERE
clause does not validate the real unfiltered plan.

Preserve the inventory, migration record, and verification results. If rollback
is necessary, use the verified restore plan; running an older binary may
reintroduce the authority paths removed by this release.

See [ADR 0006](adr/0006-governed-row-access.md) for the decision. Historical
migration guides describe their releases; their earlier statements allowing
raw SQL reads of sensitive fields do not describe Core v0.27.0.
