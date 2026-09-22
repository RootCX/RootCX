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

The agent can list/read/write/replace/delete source files and request checks.
Generated code only executes inside bubblewrap with an empty environment, its
own PID/mount/user/network namespaces and the current workspace. Build execution
has no network; dependency installation allows network but disables lifecycle
scripts. Build checks mount installed dependencies read-only; Vite gets a separate
temporary directory. Pin dependencies with `bun.lock` or `package-lock.json`,
including a separate lockfile when `backend/package.json` exists. The runner
refuses readiness if this isolation is unavailable. There is no unsafe fallback.

## Configuration

Build Core with Git installed (the Core Dockerfile includes it) and the runner:

```sh
docker build -f services/app-builder/Dockerfile -t rootcx-app-builder:<release> .
```

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
3. `waitForApplicationChange(appId, id, onProgress, signal)` follows it.
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

The SHAPP companion integration preserves its conversation in session storage,
resumes an outstanding request and refreshes only after confirmed publication.
Applications with unsaved forms can cancel `rootcx:before-application-refresh`
and refresh when their pending saves complete. Platform companion assets must be
included in template sources; otherwise rebuilding an app would remove Shappy.
The SHAPP packaging integration includes these assets in `sources.json`.

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
files and worker processes. A failure after schema application can leave an
additive schema change with the old UI. Such a run explicitly requires recovery.
Automatic publication rejects removed/redefined existing fields and required new
fields on existing collections. It does not claim that Git revert rolls back data.
Plan destructive/data-rewriting migrations separately before broadening this gate.

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
