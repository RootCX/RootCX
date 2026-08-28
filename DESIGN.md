# Delegated row ownership

A permission key is table-wide. `owner: true` already narrowed one to "the rows
whose user id column is mine". This extends that to rows that carry no user id at
all: an order line is mine because its order is, a comment because its ticket is,
a submission because its assignment is, because its enrollment is.

Enforcement stays in PostgreSQL RLS. Nothing moves into the API layer.

---

## The manifest syntax

Ownership stays one flag on one field. Its **type** decides which of the two
shapes you get:

* **Direct** — `uuid`, `text`, or `entity_link` to `core:users`: the column holds
  the user id. Unchanged from before.
* **Delegated** — `entity_link` to another entity of the *same app*: the row
  belongs to whoever owns the row it links to.

```json
{ "entityName": "enrollment", "fields": [
    { "name": "user_id", "type": "uuid", "owner": true }] },
{ "entityName": "assignment", "fields": [
    { "name": "enrollment_id", "type": "entity_link",
      "references": { "entity": "enrollment", "field": "id" }, "owner": true }] },
{ "entityName": "submission", "fields": [
    { "name": "assignment_id", "type": "entity_link",
      "references": { "entity": "assignment", "field": "id" }, "owner": true }] }
```

`app:school:submission.read.own` now reaches the submissions hanging off the
caller's own enrollments, two links away.

No new manifest key, no new section. The `entity_link` already carries its target,
already earns a foreign key and an index, and `{"owner": true}` on it reads as "the
thing it links to owns this row" without further explanation. Before this change
that combination generated `enrollment_id = <caller uuid>` — a predicate no row
could ever satisfy — so no deployed tenant can be relying on the old meaning.

### Alternatives rejected

| Shape | Why not |
| --- | --- |
| A new field key (`ownerVia: "order_id"`) | Two ways to say "this column decides who owns the row", which then have to be checked against each other. The type already distinguishes them. |
| An entity-level path (`"ownedVia": ["line_id", "order_id"]`) | Repeats, per child, a chain the parents already declare. Change the parent's ownership and every descendant's path is silently wrong. |
| A separate manifest section | Puts the fact furthest from the column it is about, and needs its own name resolution against `dataContract`. |
| Resolve the chain at query time in the API layer | Apps are untrusted and reach the same rows through `ctx.sql`. Enforcement outside RLS is not enforcement. |
| A materialised `owner_id` denormalised onto each child | Fast reads, but every write to any ancestor has to fan out, and a missed fan-out is a silent authorization bug that persists. Ownership stops being derivable and starts being a cache to keep honest. |
| A generated `owner_id` column, maintained by triggers | Same class of problem plus trigger ordering against the existing audit and hooks triggers. |
| Inline the parent lookup in the policy (`IN (SELECT id FROM parent WHERE ...)`) | The subquery runs under the *caller's* policies on the parent, so who owns a row would depend on the caller's grants, and a chain would recurse until Postgres refuses the table. See below. |

---

## The generated SQL

Direct ownership generates exactly what it did before, byte for byte.

Delegation crosses each link through a `SECURITY DEFINER` resolver in
`rootcx_system`, one per entity that some other entity defers to:

```sql
CREATE OR REPLACE FUNCTION rootcx_system."rootcx_own.school.enrollment"()
  RETURNS SETOF uuid
  LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $rootcx$
    SELECT "id" FROM "school"."enrollment"
     WHERE "user_id" = (SELECT nullif(current_setting('rootcx.user_id', true), ''))::uuid
  $rootcx$;

CREATE OR REPLACE FUNCTION rootcx_system."rootcx_own.school.assignment"()
  RETURNS SETOF uuid
  LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog AS $rootcx$
    SELECT "id" FROM "school"."assignment"
     WHERE coalesce(nullif(current_setting('rootcx.app_id', true), ''), 'school') = 'school'
       AND "enrollment_id" = ANY (ARRAY(SELECT rootcx_system."rootcx_own.school.enrollment"()))
  $rootcx$;

REVOKE ALL ON FUNCTION rootcx_system."rootcx_own.school.assignment"() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION rootcx_system."rootcx_own.school.assignment"() TO rootcx_app_executor;
```

and the table's four row-scoped policies (this is the live `pg_policies.qual`, two
links deep):

```sql
CREATE POLICY rootcx_rls_select_own ON "school"."submission" FOR SELECT USING (
  (SELECT rootcx_system.check_access('app:school:submission.read.own'))
  AND "assignment_id" = ANY (ARRAY(SELECT rootcx_system."rootcx_own.school.assignment"()))
);
-- INSERT: WITH CHECK (...)   DELETE: USING (...)
-- UPDATE: USING (...) WITH CHECK (...)  -- both, or a confined caller could
--                                       -- re-parent its row out of its own scope
```

Three decisions carry weight here.

**Why a function boundary.** Read the parent inline and Postgres applies the
parent's policies to the subquery. Two consequences, both bad: who owns a row would
start depending on what the caller may *read*, and a chain longer than one link
makes Postgres report `infinite recursion detected in policy` — at query time, on a
table that is then unusable. The resolver runs as the core role (asserted
`SUPERUSER OR BYPASSRLS` at bootstrap), so it reads the parent unfiltered and cuts
the recursion at a function boundary.

**Why a function and not a view.** A view is inlinable, so the planner would see a
real semi-join. But a view records a catalog dependency on the columns it names,
and this runtime re-runs `ALTER TABLE` on app columns at every deploy and
`DROP SCHEMA` at uninstall. A dependency that turns a routine column-type change
into a failed deploy is not worth the plan. A `LANGUAGE sql` function with a string
body records no dependency.

**Why `= ANY (ARRAY(...))` and not `IN (...)`.** Both evaluate the resolver once per
query. Only the array form is a `ScalarArrayOpExpr` the planner can drive an index
with. Measured on 6 000 child rows:

```
 = ANY (ARRAY(...))   Bitmap Heap Scan ... Bitmap Index Scan on child_parent_id_idx   0.100 ms
 IN (...)             Hash Join ... Seq Scan on child (6000 rows)                     0.623 ms
```

### Cost

A confined read is one index scan on the link column, plus one resolver evaluation
per link, each itself an index scan (on the parent's owner column, then on each
intermediate link column). All of it is `InitPlan`: once per query, not per row.
The indexes come for free — `entity_link` already gets one from
`generate_foreign_keys`, and `install_app` creates one on the owner column under
the same name, so it is a no-op rather than a duplicate.
`each_link_is_crossed_by_an_indexable_resolver` asserts the plan, not the causes,
so either way of losing it (missing index, or a predicate written non-indexably)
fails the test.

---

## Why no grant combination widens a scoped grant

1. **Ownership is a function of the data alone.** Every link is crossed by a
   `SECURITY DEFINER` resolver that bypasses RLS on the tables it reads. The set of
   rows a `.own` key reaches therefore never consults the caller's permissions on
   the chain. Holding nothing on `assignment` and `enrollment`, or holding full
   unscoped read and update on both, returns the identical row set — asserted in
   `no_grant_on_the_chain_widens_what_own_sees`.

2. **The scoped policy cannot be reached without its own key.** Each row-scoped
   policy is `check_access('X.action.own') AND <mine>`. The gate is a conjunct, so
   no other grant can satisfy the policy.

3. **Other policies are separate grants, not widenings.** The eight policies are all
   PERMISSIVE, so Postgres ORs them: `(unscoped key) OR (scoped key AND mine)`.
   A caller seeing more than its own rows is holding the *unscoped* key — a
   deliberately broader grant, granted by an admin, that already meant "all rows"
   before this feature existed. What it cannot do is make the `.own` branch match a
   row it does not own.

4. **Writes are confined at both ends.** `UPDATE` carries the predicate in `USING`
   and in `WITH CHECK`. `USING` alone would let a confined caller re-parent its own
   row under someone else's order and hand it over; `WITH CHECK` refuses that, and
   `INSERT`'s `WITH CHECK` refuses planting a row under a parent that is not the
   caller's. Both asserted.

5. **A chain cannot leave the app.** Delegation resolves only `RefTarget::Local`.
   `core:users` is the direct case; cross-app references are already refused
   globally. So an app can never make its rows resolve through another app's data.

6. **`.own` stays unforgeable.** `validate_declared_perm_key` still refuses the
   suffix on anything an app declares, so an app cannot mint a `.own` key that
   means something else.

7. **A resolver answers only its own app.** RLS predicates are evaluated as the
   *invoking* role, so `rootcx_app_executor` must hold `EXECUTE` on the resolvers —
   and every app's `ctx.sql` runs as that same role. Left there, app A could call
   `rootcx_system."rootcx_own.B.entity"()` and enumerate the primary keys of the
   caller's rows in app B. Apps are mutually untrusted, so each resolver carries a
   guard on the app the transaction belongs to:

   ```sql
   WHERE coalesce(nullif(current_setting('rootcx.app_id', true), ''), 'school') = 'school'
     AND "enrollment_id" = ANY (ARRAY(SELECT rootcx_system."rootcx_own.school.enrollment"()))
   ```

   `rootcx.app_id` is a fourth GUC posed by `set_rls_context`, folded into the
   existing single `set_config` round-trip, before the drop to the executor role —
   and `set_config` is revoked from that role, so an app cannot claim to be another
   one. `EXECUTE` is revoked from `PUBLIC` as well, so nothing else in the database
   can call a resolver at all. Asserted both ways in
   `one_app_cannot_resolve_another_s_ownership`: from its own app the resolver
   returns the caller's row, from a bystander app it returns the empty set, and the
   confined read through the owning app is unaffected.

   **Why `coalesce` is fail-open on an unset GUC.** `SET LOCAL ROLE
   rootcx_app_executor` appears exactly once in the codebase, in `begin_app_tx`,
   which always has the app schema in hand. So every path that evaluates RLS at all
   — the CRUD routes, the SQL proxy's `run_sql` and `TxSession`, the agent tools in
   `core/src/tools/`, the worker's collection ops — poses the GUC, and all five pass
   the schema of the app whose data the unit of work is for. Everything else (schema
   sync, the audit and hooks triggers, the retroactive RLS pass, the worker's own
   bookkeeping) runs on the core's superuser pool, which bypasses RLS and never
   reaches a policy. Unset therefore means "no policy is being evaluated", where
   denying buys nothing and would strand any future caller reading an app table
   directly. `nullif` is part of the same reasoning: a pooled connection that once
   served an app keeps the GUC as `''` rather than unset, and `''` means the same
   "nobody said" as absent.

   **What this constrains later.** Cross-app SQL, when it arrives, will need the
   guard keyed on a *set* of schemas the transaction is entitled to rather than the
   single one — otherwise a legitimate cross-app read of a delegated table would
   lose its `.own` branch (the unscoped branch is unaffected). Today cross-app
   entity references are refused at install, so nothing legitimate crosses.

**What this does not defend against, by construction.** An app controls the data in
its own schema, so it controls who owns what — it can point a link at anyone's
parent row. That is equally true of the direct case (the app writes `user_id`), and
it is the intended semantics: ownership *is* data. What RLS guarantees is that a
*confined caller* cannot do it, and the audit trail records who did.

---

## What is refused at install

Before any DDL runs, so a rejected install leaves nothing behind
(`a_delegation_that_cannot_terminate_is_refused` asserts the schema is absent):

| Manifest | Message |
| --- | --- |
| A chain that loops | `entity 'a' delegates ownership in a loop (a -> b -> a); a chain must end on a column holding a user id` |
| A chain ending on an entity with no owner | `entity 'comment' delegates ownership to 'ticket', which declares no owner field; mark the column that owns a 'ticket' row with "owner": true` |
| A chain spanning more than 4 entities | `entity 'link_0' delegates ownership through 5 entities (link_0 -> ... -> link_4); at most 4 are allowed` |
| An `entity_link` owner with no `references` | `entity 'profile': owner field 'user_id' is an entity_link with no 'references'; point it at 'core:users' to hold a user id, or at the entity that owns the row` |
| A resolver name past 63 bytes | `entity 'eee…' of app 'hr' needs an ownership resolver named 'rootcx_own.hr.eee…', which exceeds PostgreSQL's 63-byte identifier limit; shorten the app or entity name` |
| Two `owner` columns, or a type that cannot hold a user id | unchanged |

The loop and the dead end are the two that matter. Left to the database, a loop is
`infinite recursion detected in policy` reported the first time the table is
queried — after the deploy is declared a success, with the table unreadable until
the manifest is fixed. A dead end is quieter and worse: the policies simply match no
row, which reads as an access bug rather than as the manifest mistake it is.

The 63-byte cap is not tidiness. `rootcx_own.{schema}.{entity}` truncated by
Postgres could collide two entities into one resolver, handing one entity's rows the
other's owners. The `.` separator is deliberate: `validate_ident` bars it from both
halves, so no pair of (schema, entity) can produce another pair's name — which a
`_` separator would allow (`a` + `b_c` vs `a_b` + `c`).

**Fail-closed at runtime too.** Install-time validation cannot vouch for a
projection replayed at boot, so `owner_predicate` re-checks the bound and the loop,
and returns `None` — no row-scoped policies, and a `.own` key that grants nothing —
for a missing column, a missing primary key, a vanished parent, a loop, or a chain
too deep. Each logs a warning naming the entity.

---

## Backwards compatibility

* **An entity that declares nothing gets the pre-ownership SQL, unchanged.** A table
  absent from the `OwnerMap` produces no resolver and no `_own` policies.
  `ownership_artifacts_track_the_declaration` asserts that only the owned table
  gains them.
* **A directly-owned entity generates the identical predicate it did before.** The
  chain walk terminates immediately and the delegation code never runs.
* **The projection gains one nullable column.** `sensitive_fields.owner_parent`
  is added with `ADD COLUMN IF NOT EXISTS`, alongside `owner_field`, so a tenant
  that upgraded through the direct-ownership release gains it without a rewrite.
  NULL means "owns directly", which is what every existing row already means.
* **The boot replay reads that projection, not the stored manifest** — which is
  never revalidated after install. It now groups rows per schema first, because
  resolving a delegated entity needs the entities it defers to, not just its own
  row.
* **Both directions of a redeploy reconcile.** Dropping the declaration drops the
  policies, the `.own` keys, the projection, and now the resolvers; uninstall drops
  the resolvers too, since they live in `rootcx_system` and `DROP SCHEMA` does not
  reach them. `resolvers_track_the_declaration` asserts all three.
* **Resolvers are created on the way down, not in a pass of their own**, so a
  child's policy can never be created before the resolver it names — whatever
  order the manifest lists its entities in.

---

## Implementation

| File | Change |
| --- | --- |
| `crates/shared-types/src/lib.rs` | `FieldContract::owner` doc: the two shapes. No type change. |
| `core/src/manifest.rs` | `owner_parent`, `owner_map`, `MAX_OWNER_CHAIN`, `validate_owner_chains`; the `references` check in `validate_owner_field`; resolver pruning on uninstall. |
| `core/src/extensions/rbac/mod.rs` | `OwnerMap`, `owner_resolver_name`, the chain-walking `owner_predicate`, `declare_owner_resolver` (with the `rootcx.app_id` guard), `prune_owner_resolvers`, `column_type`, `primary_key`. |
| `core/src/governance/enforcement/sql_proxy.rs` | `set_rls_context` takes the app schema and poses `rootcx.app_id`, in the existing `set_config` round-trip. |
| `core/src/extensions/rbac/bootstrap.rs` | Boot replay groups the projection per schema. |
| `core/src/extensions/hooks.rs` | `row_shape` and `sync_sensitive_fields` carry `owner_parent`. |
| `core/src/governance/audit/audit_ext.rs` | `owner_parent` column on `sensitive_fields`. |
| skills `manifest.md` / `SKILL.md` | The two shapes, the example, what is refused. |

Key minting needed no change: it keys off `owner_field`, which is set for both
shapes.

---

## Test results

```
$ ROOTCX_RESOURCES=~/.rootcx/bin cargo test -p rootcx-core --test row_ownership_test
cargo test: 15 passed (1 suite, 37.52s)

$ cargo test -p rootcx-core --lib -- --test-threads=1
cargo test: 365 passed (1 suite, 2.98s)

$ ROOTCX_RESOURCES=~/.rootcx/bin cargo test -p rootcx-core --test governance_contract_test
cargo test: 78 passed, 1 ignored (1 suite, 148.38s)

$ cargo check --workspace
0 errors (2 pre-existing warnings)
```

`row_ownership_test` was 8 tests; the 7 added cover delegation end to end
(`own_follows_a_two_link_delegation_chain`), the no-widening property
(`no_grant_on_the_chain_widens_what_own_sees`), the generated SQL shape and its plan
(`each_link_is_crossed_by_an_indexable_resolver`), install refusal
(`a_delegation_that_cannot_terminate_is_refused`), resolver lifecycle
(`resolvers_track_the_declaration`), and the cross-app guard
(`one_app_cannot_resolve_another_s_ownership`); plus `manifest.rs` unit tests
`a_delegation_chain_must_terminate_and_stay_short` and
`a_resolver_name_that_would_be_truncated_is_refused`.

Without `ROOTCX_RESOURCES` pointing at a directory containing the `bun` binary the
harness cannot boot (`boot failed: Worker(...)`) — unrelated to this change. Under
full parallelism `platform_storage::integration` fails pre-existing; `--test-threads=1`
is clean.
