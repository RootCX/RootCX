# ADR 0008: Approve backend code to exercise bounded local data authority

Status: Accepted for Core v0.27.0.

## Context

Ordinary workers are untrusted applications whose database access follows the
caller's entity permissions and governed row access. A fine action permission
authorizes dispatch; it does not grant database access. This is appropriate for
direct CRUD and declarative ownership or assignment sharing.

Some operations require a different boundary: two users can read the same case
through a reviewed projection, while private notes remain hidden and creation
requires a live business assignment. Giving those users entity CRUD would
bypass the projection and workflow. Duplicating records per reader introduces
synchronization and revocation problems. A general policy language would move
application behavior into another runtime.

## Decision

An action may request `authority: {data: {entity: [verb, ...]}}`. The initial
scope is exact declared local entities and nonempty, unique subsets of `read`,
`create`, `update`, and `delete`. There are no wildcard scopes, cross-app
entities, predicates, policy DSL, or domain-specific action types.
Action IDs follow manifest permission-key validation; use lowercase snake_case
consistently in declarations, handlers, grants and calls.

An administrator separately approves the action against a reviewed installation
ID, release revision, complete manifest, and backend digest. Deployment and
declaration never approve automatically. Backend dependencies are installed
without lifecycle scripts before the deployed tree is hashed. Core records a
versioned snapshot containing resolved dependencies, verifies its hash at
approval and approved-process startup, and executes from that snapshot.
Read-only settings are defensive: they do not establish immutability against
hostile processes running under the same OS account or prevent changes after
verification.

The approver trusts the **full artifact**, including dependencies, initialization
and all handlers sharing its runtime. JavaScript functions and the injected
prelude do not isolate mutually untrusted code in one process. Business
authorization, safe projections and parameter validation belong to the reviewed
backend. Core enforces the action's approved data ceiling, sensitive column
privileges, SQL admission, invocation lifetime and transaction boundaries.
A per-approval PostgreSQL role and generated RLS enforce the requested local
tables, verbs and readable columns. Denied access raises database
permission/RLS errors rather than relying on application filters to hide rows.
Caller data grants do not widen the ceiling. The ceiling is entity-wide CRUD;
reader-specific row checks and redaction remain the backend's responsibility.

Approved `read` includes row locking without business update authority. The
per-approval role receives the technical `UPDATE(primary_key)` ACL required by
PostgreSQL. Generated RLS permits approved read in `UPDATE USING`, so
`SELECT ... FOR SHARE` can lock a visible row; `WITH CHECK` remains false unless
`update` is explicitly declared. Actual primary-key updates are therefore
denied under read-only authority. This separation avoids widening a guarded
create's assignment access merely to lock its authorization row.

Foreign-key referential actions can write child tables outside ordinary child
ACL/RLS checks. Approval therefore validates their complete write closure.
Cascade deletes require explicit child `delete`; update cascades and
`SET NULL`/`SET DEFAULT` effects require child `update`, recursively.
Missing child authority or a cross-app effect rejects approval. Core does not
implicitly widen authority, even if the reviewed handler intends not to
exercise the available parent mutation.

Each approved invocation gets a fresh process through the usual cross-platform
Bun worker spawning path, with a Core-bound caller, action and approval and no
injected credentials. Core checks the bound context and invocation lifetime at
the governed IPC/SQL boundary. The process is stopped after the invocation and
cannot return to an ordinary worker cache. Core permits only local
collection/SQL/transaction data messages for the live invocation. Jobs, remote
collections, self-action calls, tools, integrations, storage and event
capabilities are denied.

This preserves the existing deployment model without a new OS-identity or
volume-ownership prerequisite. Ordinary applications remain untrusted at the
governed IPC/SQL layer; the full approved artifact is trusted for business
authorization and output projection. Host filesystem and process confinement
are outside this model. A fresh process, hash verification and read-only flags
do not isolate mutually hostile programs sharing an OS account or establish
complete malicious-app confinement. Platform test coverage must be reported
from actual runs, not inferred from the cross-platform spawning architecture.

## Dispatch and review identity

An authenticated approved call requires the fine
`app:{app}:action:{id}` grant and a current approval. Plain
`app:{app}:invoke` is insufficient. Delegated calls must also retain the fine
grant in their effective permission ceiling. Authority actions cannot be
public RPCs.
An invocation without current approval returns HTTP 403. An approval POST whose
review identifiers no longer match returns HTTP 409.

Ordinary declared actions without authority retain `invoke` **OR** the fine
action grant; undeclared RPCs require `invoke`. This records the current RPC
contract, superseding the older AND wording in ADR 0003. `isolatedScope` alone
does not confer approved data authority.

Every backend deployment rotates the release revision, even for identical
bytes. Every change to the full manifest, including presentation fields, also
rotates it and revokes approvals. Installation generations separately prevent
reuse across reinstallations and contract changes. Core stores and checks the
approved manifest itself. Therefore the existing approval API can bind exact
review with `{revision, backendDigest, installationId}`; it does not need an
additional client-supplied `manifestDigest`. Restoring old code or declarations
never revives a revoked approval.

## Transactions and revocation

At database admission Core verifies current approval and caller action
permission, then holds shared locks on the installation, release and approval
for the transaction. Approval revocation waits for admitted transactions to
commit or roll back before returning. It denies subsequent transaction
admission, including from an invocation started before revocation. An already
admitted transaction may commit before the revocation completes.

Business assignments are separate. A guarded create must check and lock an
active assignment within `ctx.transaction`, then write before releasing that
lock. Assignment `read` authority suffices for the lock; only the separately
authorized revocation path needs to update/delete that same row. A check in an
earlier statement transaction or an unlocked check followed by a write leaves
a race. Core's approval lock does not decide or enforce these app-specific
rules.

## Consequences

Users can receive a small set of action grants without collection CRUD. Apps
keep one set of business records and express projections and workflows in
ordinary TypeScript/JavaScript with `serve`, `caller.userId`, `ctx.sql` and
`ctx.transaction`. Sensitive data stays outside readable authority.

Approval is an explicit trust transfer to reviewed executable code, not proof
of its correctness. Review must include dependencies and every reachable
handler; a malicious or buggy artifact can misuse its approved ceiling.
Per-invocation processes cost startup time and memory. Whole-manifest revision
rotation also makes cosmetic changes require reapproval; this is intentional
to keep the current review API unambiguous. Audit records associate execution
with approval and caller, but do not replace business validation.

The [developer recipe](../approved-actions.md) demonstrates two readers,
private notes, direct-collection denial and assignment-locked creation.
