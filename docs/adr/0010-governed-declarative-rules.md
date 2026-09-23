# ADR 0010: Governed declarative rules

Status: Accepted for the next Core release.

## Context

ADR 0006 made installation refuse app-supplied SQL: raw `checks`, index
expressions, partial-index predicates, operator classes and storage parameters.
Those strings were interpolated into DDL executed by the DDL pool, which must be
SUPERUSER or BYPASSRLS (`rbac/bootstrap.rs`). The refusal was "for now" and
offered no replacement. Applications such as an ERP lost the only way to declare
database invariants: positive quantities, percentages in range, state-dependent
required fields, date ordering, partial uniqueness.

Removing such declarations was not a safe workaround. Core drops every
constraint or index carrying its `rootcx:chk:`/`rootcx:idx:` tag that a manifest
no longer declares, so stripping them to pass admission silently deleted the
invariants from the database.

Verified threats of the raw form:

1. **Breakout.** `CHECK ({expr})` is a string template. The expression
   `true), NO FORCE ROW LEVEL SECURITY, DISABLE ROW LEVEL SECURITY, ADD
   CONSTRAINT zz CHECK (true` is one statement and disabled RLS on PostgreSQL 16.
   Index `where`, `expr`, `ops` and `with` were pasted the same way.
2. **Owner-authority functions.** PostgreSQL does not require CHECK functions to
   be immutable. Validation of existing rows ran `pg_read_file()` and
   `query_to_xml('select …')` as the DDL role.
3. **Sensitive-value oracle.** A check over a `sensitive` column reveals, through
   accept/reject, information the column privileges hide.
4. **Unanalysable dependencies.** Operator classes such as
   `kova_erp.gin_trgm_ops` bind indexes to objects in an app schema; dropping
   that schema cascades into other apps.

Rules must stay database constraints: they protect every write path, including
worker SQL and administrator imports.

## Decision

Applications declare rules in a closed language that Core parses, type-checks
and compiles itself. No author string ever reaches SQL. The legacy JSON keys stay
(`checks[].expr`, index `where`, `columns[].expr`), so existing manifests keep
their shape, but their content is now a program in this language, not SQL.

### Rule language

One boolean expression per rule, with PostgreSQL's precedence so that a legacy
string means the same thing to Core and to PostgreSQL (lowest to highest):

| Level | Construct |
| --- | --- |
| 1 | `OR` |
| 2 | `AND` |
| 3 | `NOT` (prefix) |
| 4 | `IS [NOT] NULL` (postfix) |
| 5 | `=` `<>` `!=` `<` `<=` `>` `>=` (non-associative) |
| 6 | `[NOT] BETWEEN a AND b`, `[NOT] IN (literal, …)` |
| 7 | `~` (regular-expression match) |
| 8 | binary `+` `-` |
| 9 | `*` |
| 10 | unary `-` |

Atoms: declared or system field names (`id`, `created_at`, `updated_at`),
literals (`'text'` with `''` escaping, `123`, `4.5`, `TRUE`, `FALSE`), `NULL`
only after `IS`, `interval '<n> days|hours|minutes|seconds'`, parenthesised
expressions and allow-listed calls.

Allow-listed functions, all `IMMUTABLE` in `pg_proc` (asserted by a test):
`length`, `btrim`/`trim` (one argument), `upper`, `lower`, `coalesce` (two
arguments of one type), `scale`, `trunc` (one argument), `abs`,
`jsonb_typeof`, `jsonb_array_length`. Core emits every call
`pg_catalog.`-qualified and always guards `jsonb_array_length` with
`CASE WHEN jsonb_typeof(x) = 'array' …` so a rule cannot raise.

Refused by construction: casts (`::`, `CAST`), `COLLATE`, subqueries, other
tables, qualified or quoted identifiers, system columns (`ctid`, `xmin`,
`tableoid`, …), comments, dollar quoting, escape strings, `/`, `%`, `^`, any
function outside the list, `now()` and every time- or session-dependent keyword,
special literals (`'now'`, `'today'`, `'epoch'`, `'infinity'`, `'NaN'`), and
unknown keywords.

Type checking resolves every field against the manifest:

- Numeric types (`number`, `decimal`, integer results) combine as PostgreSQL's
  implicit, immutable promotions do. Text supports `=`, `<>`, `IN` and `~`, but
  no ordering, which would depend on collation. Booleans compare with `=`/`<>`,
  so `(a) = (b)` expresses equivalence.
- Dates, timestamps and UUIDs accept text literals only in unambiguous forms
  (`YYYY-MM-DD`, RFC 3339 with an explicit offset normalized to UTC, canonical
  UUID). Dates never compare with timestamps: that cast is `STABLE`.
- `timestamp - timestamp` yields an interval comparable with an interval literal.
  Timestamp-plus-interval arithmetic is refused (DST makes it `STABLE`).
- `~` requires a string literal pattern (see below). Arrays only support
  `IS [NOT] NULL`.

Limits: 2 KiB of source, depth 24, 256 nodes, 100 `IN` items, 256-byte string
literals, 128-character patterns, 64 checks and 64 indexes per entity.

Patterns are parsed with `regex-syntax` and must be a common subset of Rust and
PostgreSQL ARE syntax: no backreferences, lookaround, inline flags, `***`
directors or `\m \M \y \A \Z` anchors, counted repetition at most 100, and no
nested quantifiers (star height at most 1). The validated pattern is emitted as a
quoted literal.

A rule referencing a `sensitive` field may reference no other field; index
predicates and index expressions may not reference sensitive fields at all.

### Field shorthands

Common single-field rules are declared on the field (snake_case like every
field key). Each compiles to a Core-named check and passes on NULL; combine with
`required` to forbid NULL.

| Key | Types | Rule |
| --- | --- | --- |
| `minimum`, `maximum`, `exclusive_minimum`, `exclusive_maximum` | number (JSON number), decimal (JSON string) | bound, plus a finiteness guard |
| `integer: true` | number, decimal | `x = trunc(x)`, finite |
| `max_scale` | decimal | `scale(x) <= n`, finite |
| `min_length`, `max_length` | text | `length(x)` bound |
| `not_blank: true` | text | `length(btrim(x)) > 0` |
| `format: "email"` | text | the email pattern Kova uses |
| `pattern` | text | restricted pattern, as above |
| `json_type` | json | `jsonb_typeof(x) = '<type>'` |
| `max_items` | json | guarded `jsonb_array_length(x) <= n` |

The finiteness guard rejects NaN and ±Infinity, which PostgreSQL otherwise
orders above every number and which `scale()` maps to NULL.

### Indexes

`where` and `columns[].expr` use the rule language (`where` must be boolean).
`ops` and `with` remain refused. `using: "trigram"` declares a trigram GIN index
on text columns; Core owns `pg_trgm` in the `rootcx_ext` schema so no app schema
holds the operator class another app depends on.

### Names, tags and compilation identity

Rule and index names are required for `checks`, snake_case, and at most 63
bytes: PostgreSQL truncates longer names, and Core used to plan `Create(long)`
and `Drop(truncated)` for the same object, leaving no constraint on every other
install. Generated names (`chk_<entity>_<field>` for enums,
`chk_<entity>_<field>_<kind>` for shorthands) are truncated to 54 bytes plus
`_` and eight hex digits of a hash when needed.

Managed objects are tagged `rootcx:chk:r1-<h>` / `rootcx:idx:r1-<h>`, where `h`
is the first 16 hex digits of SHA-256 over the grammar version, the canonical
AST and the PostgreSQL types of the referenced fields. A type change therefore
recompiles the rule. Bumping the grammar version is required whenever emission
changes meaning.

### Reconciliation

All rule work for an install runs on one dedicated session with
`search_path = pg_catalog, pg_temp`, `standard_conforming_strings = on`, a
`lock_timeout` and a `statement_timeout`, then the session is closed.

1. **Leftovers.** Objects tagged `pending` or named `rootcx_next_*` from an
   interrupted run are dropped first. They never were, or no longer are, the
   enforced version.
2. **Keep** when the tag already matches.
3. **Adopt by retag** (no table scan) when the object carries a legacy
   `rootcx:*:<fnv>` tag equal to the legacy hash of the current declaration, and
   the previously stored manifest declared the same object byte-for-byte. The
   stored comment proves which string produced the object; the language's
   PostgreSQL-compatible parse proves the new compilation means the same thing.
   Legacy trigram indexes are adopted by catalog structure (same columns, GIN,
   `gin_trgm_ops`, no predicate).
4. **Preflight** every other new or changed rule across the whole app before
   changing any constraint or index: `WHERE (rule) IS FALSE` for checks,
   duplicate groups for unique indexes. Any violation aborts the install with the
   rule, the count and up to five ids. Nothing is changed.
5. **Create or replace without a gap.** A new check is added `NOT VALID`, then
   validated. A replacement is added as `rootcx_next_<hash>` `NOT VALID` with a
   `pending` tag and committed, so new writes must satisfy both versions; it is
   validated under a lock that allows writes; then one transaction drops the old
   object, renames the new one and writes the final tag. Indexes follow the same
   shape with `CREATE INDEX CONCURRENTLY` on non-empty tables.
6. **Drop** undeclared managed objects last.

A crash between steps leaves the old rule enforced or both rules enforced; the
next install cleans the leftovers and redoes the step.

### Errors

A write rejected by a check (`23514`) returns `422` with
`{"error": "rule_violation", "entity", "rule"}`; a unique violation (`23505`)
returns `409` with `{"error": "unique_violation", "entity", "index"}`. PostgreSQL's
`DETAIL`, which contains row values, is never forwarded. Clients localize from
the rule name.

Admission errors name the rule and position, for example
`checks[ck_x].expr 1:23: function 'pg_read_file' is not allowed; allowed: …`.

### Supporting hardening

- Unknown keys on fields, checks, indexes, index columns and entities are
  refused at installation. A misspelled `maxLength` would otherwise disable a
  rule silently, as camelCase `enumValues` once did. Stored manifests and access
  projections are still read leniently at boot, so an older stored document
  never prevents startup.
- Generated CRUD refuses NaN, Infinity and unparsable numbers instead of storing
  `0`.

## Consequences

- Kova's 144 checks and 76 index declarations remain expressible. Unchanged
  strings adopt by retag without scanning tables; the nine trigram indexes need
  `using: "trigram"` instead of `using: "gin"` plus `ops`.
- Authors get line and column errors, type errors and limits at admission rather
  than PostgreSQL failures at DDL time.
- Rule changes cost a validation scan but never open a window without
  enforcement.
- Any manifest change still rotates the governance contract and revokes
  cross-app grants, as before.

## Open questions and chosen defaults

- **Contract rotation on rule changes.** Kept: adding a rule rotates the contract
  like any manifest change. Relaxing this needs its own decision.
- **Unique indexes on sensitive columns** remain an existence oracle, as before.
  Unchanged here.
- **`integer`** is a shorthand, not a new field type.
- **Collation of patterns.** New pattern rules emit no `COLLATE "C"`, which keeps
  legacy adoption exact. Character classes therefore follow the database
  collation.
- **Author messages.** `checks[].message` is not accepted yet; clients map rule
  names to messages.
- **Linting.** Warnings for NaN-tolerant comparisons or three-valued-logic traps
  in `(p) = (q)` are not surfaced yet; shorthands carry the finiteness guard.
