# Governance Refactor -- Remaining Items

Branch: `governance-refactor` (audited 2026-05-30, review 2026-06-01)
Approach: market best practice (aligned Supabase/PostgREST/Hasura pattern)

---

## Review 2026-06-01 (avant merge)

Findings de la review multi-agents + run Playwright E2E. Severite et statut.

### B1 -- Tokens bruts dans l'URL (magic-link) -- RESOLU

`magic_link/routes.rs` `consume_get` ajoutait `access_token`/`refresh_token`
en query + fragment de maniere inconditionnelle, meme quand le client demandait
la livraison par nonce -> fuite dans les logs proxy/serveur, l'historique et le
Referer. OIDC etait deja gate ; magic-link avait diverge.

**Resolu** par le commit `2988ac0` : module partage `auth/token_delivery.rs`
(source unique create/exchange/prune + `deliver`), preference `token_delivery`
stockee par token au generate (defaut `query`). Retro-compatible (SDK < 0.19
gardent les tokens en URL via le fragment) ; `token_delivery=nonce` ne met que
`auth_nonce` dans l'URL. Garde-fou de test `magic_link_nonce_delivery_keeps_tokens_out_of_url`.

### B2 -- Sandbox Layer-1 sans test executable -- RESOLU

`governance_contract_test.rs` (~4.1-4.7) : les tests d'isolation du process
worker etaient des blocs de commentaires ASCII sans execution. Desormais :

- `t4_sandbox_worker_env_has_no_secrets` : **test definitif**. Deploy un vrai
  app JS (`dumpEnv` RPC), invoke via HTTP, assert que `process.env` du worker
  ne contient ni `DATABASE_URL` ni `ROOTCX_JWT_SECRET`. Teste le vrai chemin
  `spawn_worker` -> `env_clear` -> `sandbox_env` -> bun -> IPC -> reponse.
- `t4_3_rpc_caller_wire_carries_no_token` : assert le set exact de cles
  serialisees de `RpcCaller`. Tout nouveau champ casse le test.
- `t4_7_discover_wire_carries_no_database_url` : assert le set exact de cles
  de `OutboundMessage::Discover`.
- `t4_4_fetch_core_http_without_token_is_401` : inchange.

Refactoring : `sandbox_env()` extraite dans `worker.rs` (crate-private).

### Audit-log lisible par tout utilisateur -- HIGH

`audit.rs:175` `list_audit_events` n'a aucun gate de permission : tout user
authentifie lit le journal d'audit global (old/new JSONB de toutes les apps).

**Fix :** `require_perm(&pool, identity.user_id, "admin:db.query")` (ou un
`admin:audit.read` dedie), ou scoper aux apps accessibles au caller.

### Nonces stockes en clair -- RESOLU

`auth_nonces` refactoree : la table ne stocke plus aucun token. Pattern aligne
sur l'industrie (Supabase/Hydra/Auth0) :
- Le nonce est hashe (SHA-256 via `secure_tokens::hash`) avant stockage.
- Seuls `(nonce_hash, user_id, session_id, created_at)` sont en DB.
- Les JWT sont mintes a la volee au moment de l'exchange (`token_delivery::exchange`).
- Zero token en DB, zero secret a chiffrer.

### Ecritures refusees par RLS -> HTTP 500 -- MEDIUM

`crud.rs` + `api_error.rs:31` : une ecriture refusee par RLS (`42501`) remonte
en 500 au lieu de 403 ; un DELETE non autorise renvoie 404 (masque l'echec de
permission). Contrat API casse, gestion d'erreur cliente impossible.

**Fix :** dans `From<sqlx::Error>`, mapper `db_err.code() == "42501"` vers
`ApiError::Forbidden`. (Les lectures restent silencieuses / 0 rows, correct.)

### Front : routes protegees apres logout -- MEDIUM (frontend)

Apres logout (session `null`, cookie supprime), `/app/project*` rend le shell
de l'app au lieu de rediriger vers `/app/login` (observe au browser). Les
donnees restent protegees cote core (session null), donc defense-en-profondeur,
pas une fuite.

**Fix :** middleware Next.js qui redirige les non-authentifies vers login sur
les routes `/app/project*`. Cote web, pas core.

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
| 1 | Validation format permission keys | v2 | 30 min |
| 2 | OnStartComplete IPC | v2 | 2h |
| 3 | Streaming fetch | v2 (>500 apps) | 1 jour |
| 4 | Response size cap 50MB | v2 | 2h |
| 5 | Per-app timeout config | v2 (demande client) | 4h |
| 6 | Public share data path | v2 | Design TBD |
