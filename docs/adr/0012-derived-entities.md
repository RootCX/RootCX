# ADR 0012: Derived entities

Status: Proposed for the next Core release.

## Context

An app sometimes needs a table that is not a record anyone edits but data it
recomputes from its other entities: a search index, a precomputed aggregate.
Kova needs one for its search (Kova ADR-0076): a word lexicon and a posting list
per catalogue. For its dormant catalogue that is 7.9 million words and 42.7
million postings, measured on a clone of the production tenant.

Today every entity in a manifest gets, at every install (`manifest.rs`):

1. an implicit `id UUID PRIMARY KEY`, `created_at` and `updated_at`;
2. `audit_<table>`, an `AFTER INSERT OR UPDATE OR DELETE FOR EACH ROW` trigger
   that writes one `rootcx_system.audit_log` row per changed row, with the row
   as JSON (`governance/audit/audit_ext.rs`);
3. `hooks_<table>`, a row trigger that looks up `entity_hooks` per row
   (`extensions/hooks.rs`).

For the posting list that is 42.7 million audit rows on the first build, more at
every rebuild, and about 2.7 GB of UUIDs, timestamps and a primary-key index that
nothing reads. The audit adds no accountability either: the source rows the
index is computed from are audited already, and the index can be rebuilt from
them at any time. Dropping the triggers by hand does not hold, because
`install_app` re-attaches them on every deploy.

## Decision

An entity that names its source with `"derivedFrom": "<entity>"` is a derived
entity: a table its app's worker rebuilds from that source. The source must be
an ordinary entity of the same app. One key declares it; there is no separate
flag, so a derived entity always has a source and the source always guards it.

1. **No implicit columns.** Core adds neither `id` nor `created_at` nor
   `updated_at`. Its fields and indexes are all declared; it may not name a field
   `id`, `created_at` or `updated_at`.
2. **No change triggers.** Core attaches neither the audit nor the hooks trigger,
   and drops both if an earlier install attached them.
3. **Internal to the app.** Only the app's worker reaches it, through `ctx.sql`.
   The entity lookup every other path goes through (`field_type_map`, used by the
   CRUD routes, the worker's collection API, the agent data tools, publications
   and cross-app operations) refuses it with a permission error that names the
   reason. Installation refuses a manifest that publishes it, exposes it as a
   public collection, links to it from an `entity_link`, shares it, gives it an
   owner or an identity.
4. **Guarded by its source.** Its row-level security policies, column grants and
   declared indexes are generated as for any entity, but its four policies are
   guarded by the source's keys (`<source>.read`, `.create`, `.update`,
   `.delete`), and Core mints no keys of its own. Whoever may read the articles
   may read their search index; no role needs a new key, and forgetting one
   cannot silently empty a search. `EntityContract::keys_entity` is the one place
   that answers which entity's keys guard a table, for the policies and for the
   key registry alike.

An existing table that becomes derived keeps whatever system columns it already
has; Core only stops adding them, and drops its change triggers.

## Consequences

- An app can keep a large derived table without paying an audit row per write,
  and without columns it does not use.
- The audit trail stays meaningful: it records edits to the facts, not the
  recomputation of indexes over them.
- A derived entity cannot be the target of a link: it has no `id`. Rows of a
  derived entity may link to ordinary entities.
- Only the app's worker writes a derived entity, under the caller's identity, so
  guarding writes with the source's keys gives the indexing code exactly the
  authority of whoever changed the source.
- Uninstalling the app drops derived tables with its schema, as today.

## Rejected

- **An `audit: false` flag on any entity.** It would let a business record
  escape the audit trail. Tying the exemption to a declared source, from which
  the table is rebuilt, is what makes it safe.
- **A `derived: true` flag beside `derivedFrom`.** Two keys for one fact allowed
  a source without the flag, and a derived table without a source: unguarded by
  any audited data, and with keys no role had yet. The name "derived" is kept: it
  is the established term for data recomputed from a system of record (indexes,
  caches, materialized views), and "projection" already names the row-access
  contract in this codebase.
- **Tables in a schema created out of band.** Core would not govern their
  permissions, policies or indexes, and nothing would drop them on uninstall.
- **Keeping the implicit columns.** They cost 2.7 GB on the measured index and no
  path reads them, since no route reaches a derived row by id.
