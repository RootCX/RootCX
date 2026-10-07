# Hosted application builder

The hosted builder keeps the complete application sources and Git history on the
Core persistent volume, under `sources/<appId>`. Every request follows the same
path: edit real source, typecheck/build, commit, back up, apply the manifest,
publish the backend/frontend, record the deployed commit. There is no secondary
metadata/UI-generation path.

## Components and trust boundary

Core owns repositories, authorization, durable change requests and publication.
The optional `services/app-builder` service owns model calls and isolated builds.
Core itself does not depend on a coding SDK or start an embedded coding engine.
The runner receives a bounded source snapshot over its authenticated internal
HTTP endpoint. It never mounts the Core volume or receives PostgreSQL credentials,
Core JWTs, deployment authority, or backup credentials. Use one runner per tenant;
its warm dependency cache is scoped by application ID.

Sources and binary assets are base64 in the wire envelope. Limit: 4,000 files,
64 MiB decoded, 96 MiB HTTP input. Archives are bounded and cannot contain links,
devices, traversal paths or duplicate paths. Secret files, Git internals,
dependency directories and build outputs are not source inputs. Git runs without
host configuration, hooks, filters or signing. Git attributes are not accepted,
so `git archive` cannot silently transform or omit committed inputs.

The coding engine is the pinned `@opencode/sdk` 2.0.24. Its native tool loop runs
in a dedicated Bun process inside `@anthropic-ai/sandbox-runtime` 0.0.78 Linux
isolation, with the app workspace and its own persisted home. The trusted runner
exchanges commands and events over process pipes; the SDK exposes no listening
HTTP server. Native tools, skill loading, context compaction and sessions belong
to OpenCode. Project configuration and plugin discovery are disabled; the image
provides the trusted configuration and skills. The official RootCX skill is copied from the separate
`rootcx-skills` build context without rewriting its references; the hosted prompt
only replaces local onboarding/manual publication with the existing Core pipeline.
Referenced RootCX documentation is packaged under `/opt/rootcx-docs` at build time.
The official RootCX CLI 0.17.2 is installed using its checksum-verifying installer.

A loopback proxy injects the model credential outside the agent namespace. The
agent only sees its scoped proxy credential, not the real provider key. The
application sandbox permits only that exact loopback model relay and HTTPS to
`registry.npmjs.org`. Direct sockets, private services, metadata addresses and
general Internet access are denied. Kubernetes policy separately confines the
runner pod; the sandbox does not require changes to customer tenant networking.
The final publication build runs with no network and read-only dependencies;
dependency installation uses frozen lockfiles and disables lifecycle scripts.
When final verification fails, its diagnostics return to the same OpenCode session
for correction (up to two repair turns). Core still performs normal manifest
validation and schema reconciliation, including explicitly requested deletions.

## Configuration

Build Core with Git installed (the Core Dockerfile includes it) and the runner:

```sh
docker build --build-context rootcx-skills=../rootcx-skills \
  -f services/app-builder/Dockerfile -t rootcx-app-builder:<release> .
```

Use a reviewed, pinned checkout for the skill context (initial integration:
`6a01c7e43cede4ad2f5af0f5388f404859f2b5ee`, skill version 0.7.0).
The skill remains owned by that repository; updates rebuild the runner image.

Core environment:

| Variable | Purpose |
| --- | --- |
| `ROOTCX_BUILDER_URL` | Private runner origin, e.g. `http://app-builder:9201` |
| `ROOTCX_BUILDER_TOKEN` | Random shared service credential, at least 32 characters |
| `ROOTCX_BUILDER_BACKUP_URL` | HTTPS object receiver base URL |
| `ROOTCX_BUILDER_BACKUP_TOKEN` | Credential for that receiver |

The receiver must durably store `PUT <base>/<appId>/<commit>.bundle` and only
acknowledge success after persistence with `{ "sha256": "<digest>", "bytes": N }`.
Core verifies this receipt before publication. The included `backup-receiver.mjs` implements this protocol with fsync, atomic
creation and idempotent retry. Run it separately behind TLS, with
`ROOTCX_BACKUP_DIRECTORY` on independently backed persistent storage and
`ROOTCX_BUILDER_BACKUP_TOKEN`. It is not a raw S3 bucket URL. Keep it outside the Core PVC's failure domain. Import
and publication fail closed when backup is unavailable. The explicit
`ROOTCX_BUILDER_ALLOW_UNBACKED_SOURCES=true` switch exists only for disposable
local/test environments. Source bundles contain the complete Git history.

Runner environment:

| Variable | Purpose |
| --- | --- |
| `ROOTCX_BUILDER_TOKEN` | Same internal service credential |
| `ROOTCX_BUILDER_LLM_ENDPOINT` | Full HTTPS Anthropic-compatible Messages endpoint |
| `ROOTCX_BUILDER_LLM_KEY` | Server-side provider credential, never passed to builds |
| `ROOTCX_BUILDER_MODEL` | Explicit model ID supported by that endpoint |
| `ROOTCX_BUILDER_CONCURRENCY` | 1 by default, bounded to 1–4 |
| `ROOTCX_BUILDER_STATE_DIR` | Dedicated runner PVC directory; defaults to `/tmp/rootcx-builder` for disposable development |
| `PORT` | 9201 by default |

The provider/model is explicit; the service does not select or purchase one.
Existing managed LLM endpoints can be used if they accept the Messages protocol,
tools, and the supplied workspace credential. This must be validated for the
target tenant before enabling its builder.

## Kubernetes deployment requirements

The existing persistent mount is `/data/RootCX`, not all of `/data`. Increase its
current 1 GiB allocation before importing sources. Keep `sources/`, `apps/` and
`frontends/` separate. Source directories are private to the Core UID. Snapshot
and restore the Core PVC as well as PostgreSQL; PostgreSQL backups alone do not
cover source repositories or prepared release artifacts.

Run the builder in a separate non-root pod, with no Core PVC, service-account
token, host mount, host network, or database Secret. Limit ingress to the tenant's
Core pod and TCP 9201. Allow DNS and required HTTPS provider/package-registry
egress, excluding private networks unless explicitly needed by a managed endpoint.
Give the builder CPU/memory and ephemeral-storage limits; start with 2 CPUs,
2 GiB RAM and 8 GiB ephemeral storage, then size from measured workloads.

The host runtime must allow nested unprivileged user namespaces and a fresh proc
mount. Standard restricted/seccomp profiles may reject these operations. The
startup probe intentionally catches this. Validate the selected RuntimeClass and
its user-namespace/seccomp/proc-mount policy on the target nodes. Do not solve a
failed probe by granting privileged mode or mounting the Core filesystem.

For local Docker verification, this repository was tested using a non-root
container with `--security-opt seccomp=unconfined` and
`--security-opt systempaths=unconfined`. Those flags are a local test setup, not
an assertion that the target Kubernetes policy already supports the runner.

## Import and browser use

An administrator/template publisher exports sources once:

```sh
node services/app-builder/scripts/export-sources.mjs /path/to/app /tmp/sources.json
```

Install the initial application through the existing deployment path, then POST
that JSON to `/api/v1/apps/<appId>/sources`. Its manifest must match the installed
application. Import never overwrites an existing repository. After import,
direct deployment routes reject changes to the managed application so its
published files cannot drift silently from its source history.

Browser SDK methods:

1. `getApplicationSources(appId)` obtains `headCommit`.
2. `changeApplication(appId, { requestId, baseCommit, prompt })` starts a durable run.
3. `GET /api/v1/apps/<appId>/changes/<id>/events` streams authenticated SSE state.
   The SDK polling helper remains compatible for other consumers.
4. Refresh the application only when `status === "succeeded"`.

Persist the request ID before submitting. A transport retry with the same ID and
payload returns the existing run. A different payload with that ID is rejected.
One run per application can be active; stale base revisions are rejected. Up to
four runs per Core can hold leases. Closing a browser does not cancel server work.
No source code or diagnostic log needs to be rendered in the customer UI.

In the SHAPP website service, enable `ROOTCX_HOSTED_BUILDER=true` only after Core,
the runner and backup storage are configured. Template releases must include the
`sources.json` produced by `scripts/prepare-prebuilt.mjs`. Installation imports it
after initial publication; retrying an already managed installation preserves its
customized sources instead of redeploying the template.

The SHAPP platform companion persists conversations in Core. Applications can veto
its refresh action with a cancelable `rootcx:before-application-refresh` event while
saving forms. Platform assets and the companion version belong to SHAPP; do not
include them in template sources or frontend archives.

## Recovery and operational limits

Runs persist in `rootcx_system.source_runs`. A transaction-scoped PostgreSQL
advisory lock identifies live work across Core processes. Active runs periodically
check their lease connection and cancel publication if it is lost. After a process dies,
the recovery scan marks old coding requests `interrupted`; it does not fabricate
a completion or blindly replay model calls. Interrupted publication becomes
`needs_recovery` and blocks subsequent changes to that application.

An authorized administrator can resume the exact prepared commit and artifacts:

```
POST /api/v1/apps/<appId>/changes/<runId>/retry-publication
```

A retry does not call the model or create another commit. The original requester
and recovery actor remain recorded. Source head and deployed commit are updated
only after publication succeeds. Retrying can finish a crash between Git ref
activation and the database completion record. Failed source branches remain in
Git under `refs/changes/<runId>` for diagnosis.

This is a staged publication protocol, not a transaction across PostgreSQL,
files and worker processes. A failure after schema application can leave a
schema change with the old UI. Such a run explicitly requires recovery.
Hosted changes use the standard RootCX manifest installation and schema reconciliation,
including explicitly requested field removals. There is no builder-specific additive-only
schema policy. Git revisions record source changes; they do not restore deleted database values.

Frontend artifacts are versioned and activated through a symlink rename on Unix.
Old asset files remain available for already-open clients. Backend dependencies
are prepared before the current worker is stopped, and the previous backend is
retained if later activation fails. Prepared artifacts and old releases consume
space: monitor the PVC and apply a retention policy only to versions no longer
needed for recovery. The implementation does not yet automatically prune them.

## Verification

- `cargo test -p rootcx-core --lib builder::`
- `TEST_DATABASE_URL=.../rootcx_test cargo test -p rootcx-core --test builder_integration`
- `node --test services/app-builder/test/engine.test.mjs`
- Run `test/sandbox.test.mjs` inside the Linux builder container with its actual isolation policy.

The integration test uses real Core, PostgreSQL, Git and HTTP with a deterministic
coding-service fixture. It proves source revision, schema, data and served frontend
coherence, not a particular model's reliability. A real Traiteur build was also
run inside the Linux sandbox: approximately 9.5 seconds cold and 6.5 seconds warm,
excluding model inference. These are local measurements, not production SLOs.

## Rollout status

The Kubernetes manifest is a deployment template, not an applied cluster change.
The operator must supply its image, configuration Secret and the referenced
local seccomp profile, and validate nested isolation on the actual nodes before
routing customer requests. Local activation on localhost:9100 was subsequently verified with the configured
managed model: the agent added an optional client visit date, published its schema
and frontend, and the field was checked in the browser. This development instance
uses the explicit unbacked-sources switch; it does not validate external backup.
Insufficient AI credits now produce a specific user-facing explanation.
No cloud rollout has been performed. Target-cluster isolation validation and an
external backup/restore drill remain release gates for production.

## Live feedback and reconnection

Runner-to-Core NDJSON reports allowlisted activity categories only. Core translates
these into business language and persists the latest phase/message. Ten-second
runner heartbeats refresh liveness; raw reasoning, model prose, tool arguments,
paths and outputs never enter the public progress stream. Failures use known codes
and business messages; diagnostics remain in service logs and the private run row.

The authenticated browser SSE endpoint replays the persisted state and observes
changes every 750 ms, with connection keepalives. Terminal replay closes immediately.
Shappy reconnects with bounded backoff to the same run ID; it never resubmits a
change when a stream drops. The UI shows actual activity, elapsed time (no invented
percentage), a connection notice, and a retry action for failed/interrupted requests.
The client refreshes the app only after confirmed publication, and offers an explicit
refresh button when an app vetoes automatic refresh because of unsaved work.

Mount a dedicated runner PVC at `ROOTCX_BUILDER_STATE_DIR` for OpenCode session
continuity across pod restarts. It contains source working copies and conversation
history, never the provider secret. It is distinct from Core's authoritative sources.
Warm processes and dependencies are reused per application. At most four processes
are retained; evicted sessions can resume from their persisted home. Apply a retention
policy for inactive session homes. The Node entrypoint must close engines on shutdown.

The SDK 2 migration starts fresh internal agent sessions in `opencode-v2.db` and
`*-v2.json` mappings. It does not import the older OpenCode 1 internal history.
Core source repositories, published applications, user conversations and their
recorded requests remain intact; only the agent's private working context restarts.

Additional verification:
- `node --test services/app-builder/test/opencode.integration.test.mjs` inside the Linux image runs the real SDK inside its sandbox against a deterministic Messages server. It verifies skill loading, file edits, offline-provider errors, cancellation of a running shell command without publication, and conversation resume, without paid calls.
- `test/sdk.test.mjs` exercises the published SDK through its process pipes, including persisted sessions after restart, disabled project plugins, tool events and credit errors. `test/provider-proxy.test.mjs` checks the SDK's exact beta endpoint while rejecting other paths and parameters.
- The Core integration test verifies streamed failures, terminal replay, authorization and actual column deletion with existing records.

## Repeatable development and image verification

The supported website launcher is `npm run dev:shapp`; see the website's
`docs/shapp-development.md`. It uses `make dev-core` and
`docker-compose.builder.yml`, with the production builder entrypoint. The old
manual `/tmp` launchers and Vite gateway are not needed. `DEV_DB` and
`DEV_MANAGE_DB=false` let the Core Makefile reuse an existing local database.

Development explicitly permits HTTP only to local hosts using
`ROOTCX_BUILDER_ALLOW_LOCAL_HTTP=true`; production configuration retains HTTPS.
This transport option never skips source backups. The builder accepts trusted
configuration on stdin with `--config-stdin` so local provider credentials do not
need to appear in Docker environment or temporary files.

`bash services/app-builder/scripts/verify-image.sh IMAGE` executes native engine,
sandbox, startup/authentication and Git backup restoration checks from the image,
with no source mount or external network. The image includes ripgrep so native
skill loading does not require a first-use download. CI runs the same check on
AMD64 and ARM64 with the pinned official skill revision.

`smoke-release.mjs` runs the real source-to-publication path against a selected
test instance using `ROOTCX_SMOKE_URL`, `ROOTCX_SMOKE_TOKEN` and an exported
template source file. It creates a separate application and checks idempotency,
actual schema persistence and served frontend assets. It consumes normal model
credits and retains its test application for inspection. Run it against the
candidate image digests on an EKS test tenant before promoting them. The local
image tests do not validate EKS policies or external backup disaster recovery.

To restore a source bundle, clone it into an empty directory and explicitly
`git checkout <commit-from-the-bundle-filename>`. Backup runs before activation,
so bundle HEAD can still identify the previous published revision; the prepared
revision is retained under `refs/changes/`. Verify the expected manifest after
checkout. The backup integration test covers this pre-activation state.


### Persistent Shappy conversations

`POST /api/v1/apps/:app/changes` accepts an optional `conversationId` UUID.
Core creates that conversation atomically on its first request and scopes it to
its app and requesting user. `GET /api/v1/apps/:app/conversations` lists the 100
most recent conversations; `GET /api/v1/apps/:app/conversations/:id` returns its
100 most recent requests in chronological order, including business activity.
Other users cannot retrieve a private conversation, even with deployment rights.
Existing server-side requests are grouped into a previous conversation at boot.

Conversational requests enter a bounded durable queue (32 outstanding requests
per Core). One request executes at a time, matching the default builder worker.
Queued requests use the latest published sources when admitted; uncertain HTTP
retries retain their request ID, including after the base revision changes.
The dispatcher resumes unstarted requests after restart. Interrupted execution
is reported, not silently replayed. A publication requiring recovery blocks that
application. Requests without a conversation retain the previous conflict behavior.

Each conversation selects a persisted native OpenCode session. The independent
narrator uses the configured provider with at most 20 short calls per run,
8 seconds between calls and a 6-second deadline. Narration failure never blocks
coding. Only completed explanatory text and activity categories are summarized;
tool outputs and private reasoning are excluded. Core alone confirms publication.
The UI retrieves persisted updates while active and backs off when idle; unchanged
conversation histories are not transferred again. No browser connection owns a job.

Shappy is provided by the SHAPP platform, never packaged in App sources or releases.
Configure `ROOTCX_FRONTEND_COMPANION_URL` with the trusted absolute platform loader
URL. Core adds it to normal App HTML and removes precisely recognized legacy
embedded Shappy tags from the served response. Public sharing does not load the
companion. No App files or source commits are rewritten to adopt a newer platform
version. The website builds the same platform assets for development and production;
its CORS policy permits public modules/assets to load on tenant origins. Authenticated
conversation requests remain on the tenant Core origin.
