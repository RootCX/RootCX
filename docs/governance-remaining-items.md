# Governance Refactor -- Remaining Items

Branch: `governance-refactor` (audited 2026-05-30)
Approach: market best practice (aligned Supabase/PostgREST/Hasura pattern)

---

## 1. Integrations cassees (BLOQUANT)

Les 3 integrations built-in utilisent des APIs supprimees par le refactor :
- `c.databaseUrl` (supprime de Discover IPC)
- `caller.authToken` (supprime de RpcCaller)
- `syncAllConnectedUsers()` (supprime de globalThis)

**Fichiers :** `core/resources/integrations/{gmail,google_calendar,imap_smtp}/index.ts`

**Impact :** crash au boot. Integrations 100% non-fonctionnelles.

**Fix :** migrer vers `ctx.sql()` + `ctx.selfAction()` + `ctx.collection()`.
- Supprimer `postgres(c.databaseUrl)` (plus de connexion directe)
- Remplacer tous les `db\`SQL\`` par `ctx.sql("SQL", [params])` (positional $1, $2...)
- Remplacer `caller.authToken` + `selfAction(token, ...)` par `ctx.selfAction(action, params)`
- Remplacer `syncAllConnectedUsers(caller, x)` par `ctx.selfAction("syncConnectedUsers", { actionName: x })`
- Propager `ctx` (3e argument des handlers) dans la call chain interne

**Point d'attention :** `ensureIndexes` dans onStart fait du DDL (CREATE INDEX). Le `ctx.sql()` tourne sous `rootcx_app_executor` qui n'a pas de DDL. Solutions :
- Deplacer la creation d'index dans le bootstrap core (au deploy, dans `apply_table_rls` ou equivalent)
- Ou ajouter un chemin DDL self-schema au boot (onStart privileged path)

**Effort :** 1-2 jours (~50 requetes SQL a convertir par integration)

---

## 2. Protection resource exhaustion (BLOQUANT)

### Probleme

`sql_proxy.rs:147` fait `fetch_all` (charge tout en memoire du core). Une app malveillante envoie une requete retournant des millions de lignes -> OOM du core -> toutes les apps tombent. C'est le CORE qui alloue, pas le worker (le worker n'a pas de connexion DB).

### Solution (market practice Supabase/PostgREST)

Trois couches, exactement comme Supabase :

**A. Statement timeout (protection principale)**

`SET LOCAL statement_timeout` dans chaque transaction app. Postgres tue la requete cote serveur avant que les lignes arrivent en memoire Rust. Non-bypassable par l'app (SET bloque par validate_sql, set_config revoque, single-statement enforce par sqlx extended protocol).

Valeurs par tier (standard industrie) :

| Contexte | Timeout | Justification |
|----------|---------|---------------|
| App non-fiable (ctx.sql) | 8s | CRUD standard, meme default que Supabase `authenticated` |
| Agent tool call (query_data/mutate_data) | 30s | Agents font parfois de l'analytique |
| Admin introspection (/db/query) | Pas concerne | Endpoint separe, pool owner, pas de proxy |

Configurable par app : l'admin peut donner 60s a une app BI specifique (decision de gouvernance stockee dans le manifest ou config app).

**B. Row cap (defense en profondeur)**

Baisser MAX_ROWS de 10 000 a **1 000** par defaut (standard Supabase PostgREST `max-rows`). Configurable par app jusqu'a 10 000 max.

Deplacer le check AVANT `tx.commit()` pour que les DML trop gros soient rollback :
```rust
let rows = q.fetch_all(&mut *tx).await?;
if rows.len() > max_rows {
    // tx dropped sans commit = rollback implicite
    return Err(format!("query returned {} rows, exceeds limit {}; add LIMIT or paginate", rows.len(), max_rows));
}
tx.commit().await?;
```

**C. Idle transaction timeout (protection contre les tx zombies)**

`SET LOCAL idle_in_transaction_session_timeout = '30000'` (30s). Empeche une transaction ouverte de bloquer des locks indefiniment.

### Pourquoi l'app ne peut PAS contourner

| Vecteur | Bloque par |
|---------|-----------|
| `SET LOCAL statement_timeout = '0'` | validate_sql rejette tout SQL commencant par `SET` |
| `SELECT set_config('statement_timeout', '0', true)` | REVOKE EXECUTE on set_config FROM executor + PUBLIC |
| Multi-statement `SET ...; SELECT ...` | sqlx extended protocol = single statement |
| Persister un SET cross-requete | Chaque ctx.sql() = sa propre transaction BEGIN...COMMIT |

### Implementation concrete (sql_proxy.rs, fn begin_app_tx)

Apres `set_rls_context` et avant `SET LOCAL ROLE` :
```rust
let timeout_ms = timeout_ms.unwrap_or(8000);
sqlx::query(&format!("SET LOCAL statement_timeout = '{timeout_ms}'"))
    .execute(&mut *tx).await?;
sqlx::query("SET LOCAL idle_in_transaction_session_timeout = '30000'")
    .execute(&mut *tx).await?;
```

Ajouter un parametre `timeout_ms: Option<u32>` a `begin_app_tx`. Les callers passent :
- `None` (default 8s) pour les apps via SqlQuery IPC
- `Some(30_000)` pour les agent tools (query_data/mutate_data)
- L'endpoint admin n'utilise pas `begin_app_tx` (pool owner direct)

---

## 3. Hardening futur (pas bloquant, v2)

### 3a. Virgule dans les cles de permission

`sql_proxy.rs:58` encode les permissions en CSV (`join(",")`). Si une cle contient une virgule, le decodage cote Postgres la corrompt. Fail-closed (perte d'acces, pas gain). Aucune permission existante n'a de virgule.

**Fix preventif :** valider le format des cles au parse du manifest (rejeter tout caractere hors `[a-z0-9_:.*]`).

### 3b. Race condition onStart

Si un RPC arrive pendant que le worker execute onStart, `onstart_done` passe a true et les `ctx.collection()` restants du onStart sont refuses. Aucune integration actuelle n'est affectee (elles utilisent `db` direct, pas `ctx.collection()` dans onStart).

**Fix futur :** attendre un message IPC `OnStartComplete` du worker avant de set `onstart_done = true`.

### 3c. Streaming fetch (scalability)

Remplacer `fetch_all` par un fetch streaming (row-by-row avec abort a MAX_ROWS+1). Borne la memoire du core a O(MAX_ROWS) par requete quel que soit le resultat reel. Pertinent quand >500 apps concurrentes.

### 3d. Response size cap

Ajouter un cap en bytes sur la reponse serialisee (50 MB). Protege contre les lignes individuellement enormes (JSONB blobs). Mesurer pendant la serialisation, abort si depasse.

### 3e. Per-app timeout configurable

Stocker un `query_timeout_ms` dans la config app (manifest ou table system). L'admin configure via l'API. Le core le lit au dispatch et le passe a `begin_app_tx`. Permet de donner 60s a une app BI sans ouvrir la porte aux autres.

---

## Resume decision

| # | Item | Quand | Effort |
|---|------|-------|--------|
| 1 | Migrer les 3 integrations | Avant merge | 1-2 jours |
| 2A | statement_timeout (8s/30s) | Avant merge | 15 min |
| 2B | Row cap 1000 + check avant commit | Avant merge | 10 min |
| 2C | idle_in_transaction_session_timeout | Avant merge | 5 min |
| 3a | Validation format permission keys | v2 | 30 min |
| 3b | OnStartComplete IPC | v2 | 2h |
| 3c | Streaming fetch | v2 (>500 apps) | 1 jour |
| 3d | Response size cap 50MB | v2 | 2h |
| 3e | Per-app timeout config | v2 (demande client) | 4h |
