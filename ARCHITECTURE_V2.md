# Plan: Refactor V2 — Architecture Découplée

## Context

L'architecture actuelle est monolithique : tout vit dans le process Tauri Studio. On découple en 3 entités :

```
┌─────────────────────────────────────────────────────────┐
│                   USER DESKTOP                          │
├──────────────┬──────────────┬───────────────────────────┤
│  STUDIO      │  APP "CRM"   │  APP "POS"               │
│  (IDE/Forge) │  (UI only)   │  (UI only)               │
│              │              │                           │
│  @rootcx/    │  @rootcx/    │  @rootcx/                │
│  runtime     │  runtime     │  runtime                 │
│  (admin)     │  (hooks)     │  (hooks)                 │
└──────┬───────┴──────┬───────┴──────────┬────────────────┘
       │              │                  │
       │  fetch() to http://localhost:9100
       ▼              ▼                  ▼
┌─────────────────────────────────────────────────────────┐
│           ROOTCX RUNTIME daemon (:9100)                 │
│              crates/runtime (Rust)                       │
│                                                         │
│  ┌─────────────┐ ┌──────────────┐ ┌──────────────────┐ │
│  │ Manifest    │ │ Collections  │ │ Governance       │ │
│  │ Engine      │ │ CRUD API     │ │ (permissions,    │ │
│  │ (parse →    │ │ (REST sur    │ │  audit logs)     │ │
│  │  CREATE     │ │  les tables  │ │  [future]        │ │
│  │  TABLE)     │ │  des apps)   │ │                  │ │
│  └─────────────┘ └──────────────┘ └──────────────────┘ │
│                        │                                │
│                  ┌─────▼─────┐                          │
│                  │ PostgreSQL │                          │
│                  │ (port 5480)│                          │
│                  └───────────┘                          │
└─────────────────────────────────────────────────────────┘
```

### Séparation des responsabilités

| Entité | Rôle | Possède | NE fait PAS |
|--------|------|---------|-------------|
| **RootCX Runtime** | Le moteur. Un daemon Rust + un package npm. | Postgres lifecycle, schema bootstrap, manifest engine, Collections CRUD API, (futur: permissions, audit) | Pas d'UI, pas de Forge IA, pas de build d'apps |
| **RootCX Studio** | L'IDE. Builder d'apps IA. | Forge sidecar (IA), AppRunner (dev builds), interface admin | Pas de Postgres, pas de schema, pas de data |
| **Apps** | Les satellites. UI pure. | React UI + hooks `@rootcx/runtime` | Pas de backend, pas de DB |

### Le Runtime = 1 concept, 2 implémentations

| Composant | Techno | Rôle |
|-----------|--------|------|
| `crates/runtime` | Rust (axum) | Daemon HTTP — Postgres, API, manifest engine |
| `packages/runtime` | TypeScript (npm `@rootcx/runtime`) | Hooks React — `useAppCollection`, `useAppRecord`, etc. Appellent le daemon via `fetch()` |

---

## Phase 1 — Créer le Runtime daemon

### 1.1 Renommer `crates/kernel/` → `crates/runtime/`

| Action | Détail |
|--------|--------|
| Renommer dossier | `crates/kernel/` → `crates/runtime/` |
| `crates/runtime/Cargo.toml` | `name = "rootcx-runtime"` |
| `/Cargo.toml` | members: `"crates/runtime"`, dep: `rootcx-runtime = { path = "crates/runtime" }` |
| `apps/studio-desktop/src-tauri/Cargo.toml` | `rootcx-kernel` → `rootcx-runtime` |
| Tous les `use rootcx_kernel::` | → `use rootcx_runtime::` |

### 1.2 Déplacer les modules Studio-only

| Source | Destination |
|--------|-------------|
| `crates/runtime/src/forge.rs` | `apps/studio-desktop/src-tauri/src/forge.rs` |
| `crates/runtime/src/app_runner.rs` | `apps/studio-desktop/src-tauri/src/app_runner.rs` |

Supprimer de `crates/runtime/src/lib.rs` : `pub mod forge`, `pub mod app_runner`, les `pub use` associés.

Ajouter dans `apps/studio-desktop/src-tauri/src/lib.rs` : `mod forge; mod app_runner;`

### 1.3 Nettoyer le struct Runtime

`crates/runtime/src/lib.rs` — le struct final :

```rust
pub struct Runtime {
    pg: PostgresManager,
    pool: Option<PgPool>,
}
// Plus de forge: Option<ForgeManager>
// boot(): init_db → start PG → connect pool → bootstrap schema
// shutdown(): close pool → stop PG
// status(): OsStatus (forge = Offline)
// pool(): Option<&PgPool>
```

Renommages :
- `struct Kernel` → `struct Runtime`
- `KernelError` → `RuntimeError` (dans `error.rs`, `schema.rs`, `manifest.rs`)

### 1.4 Adapter Studio `state.rs`

```rust
pub struct AppState {
    runtime: Arc<Mutex<Runtime>>,
    forge: Option<Arc<Mutex<ForgeManager>>>,      // from crate::forge
    running_apps: Arc<Mutex<HashMap<String, AppRunner>>>,  // from crate::app_runner
}
```

- `boot()` : runtime.boot() puis forge.start()
- `shutdown()` : forge.stop() puis runtime.shutdown()
- `status()` : combine runtime.status() + forge status

### 1.5 Ajouter le serveur HTTP axum au crate runtime

**Nouveaux fichiers :**

| Fichier | Contenu |
|---------|---------|
| `crates/runtime/src/main.rs` | Entry point daemon : boot + serve + graceful shutdown |
| `crates/runtime/src/server.rs` | axum Router + `serve(runtime, port)` |
| `crates/runtime/src/routes.rs` | Handlers HTTP |
| `crates/runtime/src/api_error.rs` | ApiError enum + IntoResponse |

**Cargo.toml** ajouts : `axum`, `tower-http` (cors), section `[[bin]]`

**Routes API :**

| Méthode | Route | Description |
|---------|-------|-------------|
| `GET` | `/health` | `{"status":"ok"}` |
| `GET` | `/api/v1/status` | OsStatus |
| `POST` | `/api/v1/apps` | Install app (body: AppManifest) |
| `GET` | `/api/v1/apps` | List installed apps |
| `DELETE` | `/api/v1/apps/:app_id` | Uninstall app |
| `GET` | `/api/v1/apps/:app_id/collections/:entity` | List records |
| `POST` | `/api/v1/apps/:app_id/collections/:entity` | Create record |
| `GET` | `/api/v1/apps/:app_id/collections/:entity/:id` | Get record |
| `PATCH` | `/api/v1/apps/:app_id/collections/:entity/:id` | Update record |
| `DELETE` | `/api/v1/apps/:app_id/collections/:entity/:id` | Delete record |

### Vérification Phase 1

```bash
cargo build
cargo run -p rootcx-runtime           # daemon standalone
curl localhost:9100/health             # {"status":"ok"}
curl localhost:9100/api/v1/apps        # []
cargo tauri dev                        # Studio fonctionne (mode embedded)
```

---

## Phase 2 — Studio client + package `@rootcx/runtime`

### 2.1 Crate `crates/runtime-client/`

Client HTTP Rust pour Studio :

```rust
pub struct RuntimeClient { base_url: String, client: reqwest::Client }
impl RuntimeClient {
    pub async fn is_available(&self) -> bool
    pub async fn status(&self) -> Result<OsStatus>
    pub async fn install_app(&self, manifest: &AppManifest) -> Result<String>
    pub async fn list_apps(&self) -> Result<Vec<InstalledApp>>
    pub async fn uninstall_app(&self, app_id: &str) -> Result<()>
}
```

### 2.2 Studio en mode dual

`AppState` :
```rust
runtime_client: Option<RuntimeClient>,       // si daemon dispo
runtime: Option<Arc<Mutex<Runtime>>>,         // sinon embedded
```

`from_tauri()` : check daemon → mode API ou fallback embedded.

### 2.3 Package `packages/runtime/`

```
packages/runtime/
├── package.json          # name: "@rootcx/runtime"
├── tsconfig.json
└── src/
    ├── index.ts
    ├── client.ts         # class RuntimeAPI { fetch wrapper }
    └── hooks/
        ├── useAppCollection.ts    # CRUD hook sur une collection
        ├── useAppRecord.ts        # Get/update/delete un record
        └── useRuntimeStatus.ts    # Status du daemon
```

**`useAppCollection(appId, entityName)`** retourne :
```typescript
{ data: T[], loading, error, refetch, create(data), update(id, data), remove(id) }
```

Appels internes : `fetch("http://localhost:9100/api/v1/apps/{appId}/collections/{entity}")`

### Vérification Phase 2

```bash
cargo run -p rootcx-runtime    # daemon
cargo tauri dev                # Studio → mode API
# install app, list apps → passe par HTTP
# sans daemon → fallback embedded
```

---

## Fichiers — résumé complet

### Créés
| Fichier | Phase |
|---------|-------|
| `crates/runtime/src/main.rs` | 1 |
| `crates/runtime/src/server.rs` | 1 |
| `crates/runtime/src/routes.rs` | 1 |
| `crates/runtime/src/api_error.rs` | 1 |
| `apps/studio-desktop/src-tauri/src/forge.rs` *(déplacé)* | 1 |
| `apps/studio-desktop/src-tauri/src/app_runner.rs` *(déplacé)* | 1 |
| `crates/runtime-client/Cargo.toml` + `src/lib.rs` | 2 |
| `packages/runtime/package.json` + `src/**` | 2 |

### Modifiés
| Fichier | Phase |
|---------|-------|
| `crates/runtime/Cargo.toml` | 1 |
| `crates/runtime/src/lib.rs` | 1 |
| `crates/runtime/src/error.rs` | 1 |
| `crates/runtime/src/schema.rs` | 1 |
| `crates/runtime/src/manifest.rs` | 1 |
| `/Cargo.toml` | 1+2 |
| `apps/studio-desktop/src-tauri/Cargo.toml` | 1+2 |
| `apps/studio-desktop/src-tauri/src/lib.rs` | 1 |
| `apps/studio-desktop/src-tauri/src/state.rs` | 1+2 |
| `apps/studio-desktop/src-tauri/src/commands.rs` | 1+2 |

### Inchangés
- `crates/postgres-mgmt/`, `crates/shared-types/`, `packages/ai-forge/`, `apps/studio-ui/`

---

## Phases futures

| Phase | Description |
|-------|-------------|
| 3 | Supprimer mode embedded, Runtime en daemon launchd/systemd |
| 4 | AI Forge → data via `@rootcx/runtime` hooks (plus asyncpg) |
| 5 | Governance : permissions, audit logs |
| 6 | Template apps générées (Tauri shell + `@rootcx/runtime`) |
| 7 | Studio UI migre vers `@rootcx/runtime` hooks |
