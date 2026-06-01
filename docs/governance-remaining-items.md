# Governance Refactor -- Remaining Items

Branch: `governance-refactor` (audited 2026-05-30, review 2026-06-01)
Approach: market best practice (aligned Supabase/PostgREST/Hasura pattern)

---

## Avant merge

### Audit-log lisible par tout utilisateur -- HIGH -- RESOLU (0526a86)

`audit.rs:175` `list_audit_events` n'avait aucun gate de permission : tout user
authentifie lisait le journal d'audit global (old/new JSONB de toutes les apps).

**Fix livre :** endpoint gate derriere `admin:audit.read`.

### Ecritures refusees par RLS -> HTTP 500 -- MEDIUM -- RESOLU

`api_error.rs` : une ecriture refusee par RLS (`42501`, INSERT ou violation
WITH CHECK sur UPDATE) remontait en 500 au lieu de 403.

**Fix livre :** `From<sqlx::Error>` mappe `db_err.code() == "42501"` vers
`ApiError::Forbidden`. Couvert par `t1_3` (create sans `.create` -> 403). Les
lectures et les DELETE/UPDATE filtres par RLS USING restent volontairement
silencieux (0 rows / 404) pour ne pas divulguer l'existence des lignes.

### Front : routes protegees apres logout -- MEDIUM (frontend) -- ECARTE (non bloquant)

Apres logout (session `null`, cookie supprime), `/app/project*` rend le shell
de l'app au lieu de rediriger vers `/app/login` (observe au browser). Les
donnees restent protegees cote core (session null), donc defense-en-profondeur,
pas une fuite -- ne bloque pas le merge.

**Fix (cote web, si repris) :** middleware Next.js qui redirige les
non-authentifies vers login sur les routes `/app/project*`.

---

## Hardening futur (pas bloquant, v2)

### 1. Virgule dans les cles de permission

`sql_proxy.rs:58` encode les permissions en CSV (`join(",")`). Si une cle contient une virgule, le decodage cote Postgres la corrompt. Fail-closed (perte d'acces, pas gain). Aucune permission existante n'a de virgule.

**Fix preventif :** valider le format des cles au parse du manifest (rejeter tout caractere hors `[a-z0-9_:.*]`).

### 2. Race condition onStart

Si un RPC arrive pendant que le worker execute onStart, `onstart_done` passe a true et les `ctx.collection()` restants du onStart sont refuses. Aucune integration actuelle n'est affectee.

**Fix futur :** attendre un message IPC `OnStartComplete` du worker avant de set `onstart_done = true`.

### 3. Streaming fetch (scalability)

Remplacer `fetch_all` par un fetch streaming (row-by-row avec abort a MAX_ROWS+1). Borne la memoire du core a O(MAX_ROWS) par requete quel que soit le resultat reel. Pertinent quand >500 apps concurrentes.

### 4. Response size cap

Ajouter un cap en bytes sur la reponse serialisee (50 MB). Protege contre les lignes individuellement enormes (JSONB blobs). Mesurer pendant la serialisation, abort si depasse.

### 5. Per-app timeout configurable

Stocker un `query_timeout_ms` dans la config app (manifest ou table system). L'admin configure via l'API. Le core le lit au dispatch et le passe a `begin_app_tx`. Permet de donner 60s a une app BI sans ouvrir la porte aux autres.

### 6. Public share data path (share-token reads)

Public/anonymous RPCs (share-token or unauthenticated) produce `user_id = None` in the
RLS context, which means `check_access` denies every row. A publicly shared board, document, or
page cannot read any data via `ctx.sql` or `ctx.collection`.

Options to evaluate later:
- (A) Keep deny-all: public RPCs are stateless, data must be embedded in the share payload.
- (B) Public data scope: a dedicated RLS policy on rows marked "shared", keyed by scope in the token.
- (C) Creator-identity delegation: the share-token carries the creator's identity (read-only, scoped).
  Consistent with "a human is always responsible" but breaks if creator loses access.

Decision deferred to v2.

---

## Resume

| # | Item | Quand | Effort |
|---|------|-------|--------|
| 1 | Audit-log permission gate | RESOLU (0526a86) | -- |
| 2 | RLS 42501 -> 403 | RESOLU | -- |
| 3 | Front logout redirect | ECARTE (non bloquant, web) | -- |
| 4 | Validation format permission keys | v2 | 30 min |
| 5 | OnStartComplete IPC | v2 | 2h |
| 6 | Streaming fetch | v2 (>500 apps) | 1 jour |
| 7 | Response size cap 50MB | v2 | 2h |
| 8 | Per-app timeout config | v2 (demande client) | 4h |
| 9 | Public share data path | v2 | Design TBD |
