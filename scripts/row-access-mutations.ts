#!/usr/bin/env bun
/**
 * Reproducible, destructive mutation checks for a DISPOSABLE Docker source copy.
 *
 * Build docker/Dockerfile.test (COPY . .), use a private test PostgreSQL service,
 * and override the image entrypoint with bun. Inside that container:
 *
 *   ROOTCX_MUTATION_SANDBOX=1 bun scripts/row-access-mutations.ts [mutant-id ...]
 *
 * NEVER bind-mount a checkout at /src, an ancestor, or a descendant. The runner
 * checks Linux mountinfo, an overlay-backed /, /.dockerenv, real paths, and the
 * explicit environment opt-in before writing sources. The normal test Compose
 * image copies sources and mounts only /cache; do not add a source volume.
 * Mutant artifacts use a separate Cargo target directory; the package cache is
 * cleaned before each baseline so a previous mutant cannot pass as source.
 * Do not run alongside another mutation command using that target directory.
 *
 * TEST_DATABASE_URL must name rootcx_test on the isolated test PostgreSQL
 * service. This runner does not provision or tear down that service. Use the
 * existing pinned test image/toolchains and private Compose project lifecycle.
 *
 * Every selected test must pass BEFORE any mutation. There is no skip-baseline
 * option. Baseline compilation includes all selected Cargo test targets.
 * Mutants default to governance_test; standalone targets are explicitly mapped
 * below. Each mutant then gets a separate --no-run compile and exact-name
 * substring filter, with one test required in its result. No test source or own
 * membership expression is changed, and no Git command is used.
 *
 * stdout is one JSON report, also saved with full logs under the printed /tmp
 * directory. Progress goes to stderr. Exit 0 requires every mutant to be killed
 * at its intended assertion and every edited source to be restored. Compile,
 * boot, timeout, source drift and unrelated assertion failures are NOT kills.
 * SIGINT/SIGTERM stop the process group and unwind through source restoration.
 * SIGKILL/power loss cannot run finally: discard the container after either.
 *
 * To reproduce one result, pass its ID; the passing baseline is still required.
 * Patch anchors and assertion lines are resolved from the copied source, and
 * SHA-256 hashes, commands, tool versions and evidence are recorded in JSON.
 * Copy the report/log directory out before removing the disposable container.
 */

import { spawn, type ChildProcess } from "node:child_process";
import { createHash } from "node:crypto";
import {
  closeSync,
  existsSync,
  lstatSync,
  mkdtempSync,
  openSync,
  readFileSync,
  realpathSync,
  statSync,
  unlinkSync,
  writeFileSync,
} from "node:fs";
import { join } from "node:path";

const ROOT = "/src";
const COMPILE_MS = 20 * 60_000;
const TEST_MS = 2 * 60_000;
const MAX_LOG_BYTES = 32 * 1024 * 1024;
const LOCK = "/tmp/rootcx-row-access-mutations.lock";
const CARGO = ["cargo", "test", "--locked", "-p", "rootcx-core"];
const TARGET_SOURCES = {
  governance_test: "core/tests/governance_test.rs",
  worker_lifecycle_test: "core/tests/worker_lifecycle_test.rs",
  app_migrations_test: "core/tests/app_migrations_test.rs",
} as const;
const GOVERNANCE_SOURCES: Record<string, string> = {
  assignment_access_test: "core/tests/governance/assignment_access_test.rs",
  resource_sharing_test: "core/tests/governance/resource_sharing_test.rs",
  sensitive_sql_test: "core/tests/governance/sensitive_sql_test.rs",
  manifest_sql_admission_test: "core/tests/governance/manifest_sql_admission_test.rs",
  row_access_projection_test: "core/tests/governance/row_access_projection_test.rs",
};
type TestTarget = keyof typeof TARGET_SOURCES;
const SHARING = "core/src/governance/row_access/sharing.rs";
const POLICIES = "core/src/governance/row_access/policies.rs";
const ROW_ACCESS = "core/src/governance/row_access/mod.rs";
const BOOTSTRAP = "core/src/extensions/rbac/bootstrap.rs";
const SQL_PROXY = "core/src/governance/enforcement/sql_proxy.rs";
const MANIFEST = "core/src/manifest.rs";

type Edit = { file: string; before: string; after: string };
type Assertion = { anchor: string; marker: RegExp };
type Mutant = {
  id: string;
  description: string;
  target?: TestTarget;
  test: string;
  edits: Edit[];
  assertions: Assertion[];
};
type Evidence = { file: string; line: number; marker: RegExp; equality: boolean };
type Phase = {
  command: string[];
  log: string;
  elapsedMs: number;
  exitCode: number | null;
  signal: string | null;
  timedOut: boolean;
  interrupted: boolean;
  error?: string;
};
type Outcome =
  | "not_run" | "killed" | "survived" | "compile_error" | "boot_error"
  | "timeout" | "interrupted" | "unclassified_error" | "restore_error";
type Result = {
  id: string;
  description: string;
  target: TestTarget;
  test: string;
  outcome: Outcome;
  patches: Edit[];
  edits: { file: string; beforeSha256: string; afterSha256: string }[];
  intendedAssertions: { file: string; line: number; marker: string }[];
  compile?: Phase;
  execution?: Phase;
  evidence?: { file: string; line: number; message: string };
  error?: string;
  restored?: boolean;
};

const mutants: Mutant[] = [
  {
    id: "resource-subject-tautology",
    description: "Drop the exact resource match while retaining the active grantee and target permission.",
    test: "resource_sharing_test::resource_reads_are_exact_roots_and_explicit_multihop_targets_with_safe_projection",
    edits: [{
      file: SHARING,
      before: "quote_ident(&share.subject), target.resource,",
      after: 'quote_ident(&share.subject), format!("a.{}", quote_ident(&share.subject)),',
    }],
    assertions: [{
      anchor: `assert_eq!(
                sql_ids(&body["result"]),
                expected,
                "{entity}/tx={transaction}: sibling, self and undeclared descendants stay private"
            );`,
      marker: /project\/tx=false: sibling, self and undeclared descendants stay private/,
    }],
  },
  {
    id: "resource-active-when-tautology",
    description: "Retain resource scope but let ended assignments authorize resource reads.",
    test: "resource_sharing_test::committed_resource_revocation_reaches_the_next_statement_in_a_bun_callback",
    edits: [{
      file: SHARING,
      before: `SELECT {} FROM {}.{} a, {}, {}
                 WHERE a.{} = {} AND a.{} = {} AND a.{} IS NULL AND {} =`,
      after: `SELECT {} FROM {}.{} a, {}, {}
                 WHERE a.{} = {} AND a.{} = {} AND (a.{} IS NULL OR TRUE) AND {} =`,
    }],
    assertions: [{
      anchor: `assert!(
            sql_ids(&body["after"]).is_empty(),
            "delete={delete}: stale shared resource: {body}"
        );`,
      marker: /delete=false: stale shared resource:/,
    }],
  },
  {
    id: "omit-package-script-refusal",
    description: "Allow package install scripts to run with Core authority during deployment.",
    target: "app_migrations_test",
    test: "deploy_installs_dependencies_without_running_package_scripts",
    edits: [{
      file: "core/src/routes/deploy.rs",
      before: '        .arg("--ignore-scripts")\n',
      after: "",
    }],
    assertions: [{
      anchor: 'assert!(!root_marker.exists(), "root package scripts must not run with Core authority");',
      marker: /root package scripts must not run with Core authority/,
    }],
  },
  {
    id: "no-user-retains-owner-authority",
    description: "Let lifecycle and anonymous workers retain the pool's RLS-bypassing role.",
    target: "worker_lifecycle_test",
    test: "bun_lifecycle_and_anonymous_workers_cannot_read_or_fabricate_assignments",
    edits: [{
      file: SQL_PROXY,
      before: '    sqlx::query("SET LOCAL ROLE rootcx_app_executor").execute(&mut *tx).await?;',
      after: `    if state.user_id.is_some() {
        sqlx::query("SET LOCAL ROLE rootcx_app_executor").execute(&mut *tx).await?;
    }`,
    }],
    assertions: [{
      anchor: `assert_eq!(
                result[operation]["value"],
                json!([]),
                "{principal} {operation}: {result}"
            );`,
      marker: /lifecycle read:/,
    }],
  },
  {
    id: "execute-pending-sql-before-refusal",
    description: "Execute the first pending SQL file before returning the existing refusal.",
    target: "app_migrations_test",
    test: "pending_sql_is_refused_before_any_statement_or_bookkeeping",
    edits: [{
      file: "core/src/app_migrations.rs",
      before: '    Err(format!(\n        "app-supplied SQL migrations are disabled; pending files: {}.',
      after: `    let sql = std::fs::read_to_string(app_dir.join("migrations").join(&files[0]))
        .map_err(|error| format!("read pending migration: {error}"))?;
    sqlx::raw_sql(&sql)
        .execute(pool)
        .await
        .map_err(|error| format!("execute pending migration: {error}"))?;
    Err(format!(
        "app-supplied SQL migrations are disabled; pending files: {}.`,
    }],
    assertions: [{
      anchor: 'assert_eq!(count, 0, "not even the first pending statement may execute");',
      marker: /not even the first pending statement may execute/,
    }],
  },
  {
    id: "expose-sensitive-owner-through-resolver",
    description: "Admit a shared target whose ownership keys are sensitive, exposing them through its callable resolver.",
    test: "assignment_access_test::invalid_share_declarations_are_rejected_before_any_install_artifacts",
    edits: [{
      file: SHARING,
      before: "if owned.fields.iter().any(|field| field.owner && field.sensitive) {",
      after: "if false && owned.fields.iter().any(|field| field.owner && field.sensitive) {",
    }],
    assertions: [{
      anchor: `assert!(
            matches!(
                status,
                StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY
            ),
            "{label} must be an install error: {status} {body}"
        );`,
      marker: /sensitive direct owner must be an install error:/,
    }],
  },
  {
    id: "swap-assignment-direction",
    description: "Swap grantee and subject link columns in the shared-identity SQL.",
    test: "assignment_access_test::sharing_is_an_exact_many_to_many_core_identity_read_union",
    edits: [{
      file: SHARING,
      before: `grantee.from, subject.from, quote_ident(&share.grantee), grantee.pk,
            quote_ident(&share.subject), subject.pk,`,
      after: `grantee.from, subject.from, quote_ident(&share.subject), grantee.pk,
            quote_ident(&share.grantee), subject.pk,`,
    }],
    assertions: [{
      anchor: `assert_eq!(
                    row_ids(&body["result"]),
                    expected,
                    "helper {i}/{entity}/tx={transaction}: {body}"
                );`,
      marker: /helper 0\/person\/tx=false:/,
    }],
  },
  {
    id: "active-when-tautology",
    description: "Treat ended assignments as active, preserving the SQL format arguments.",
    test: "assignment_access_test::ending_or_deleting_the_last_assignment_revokes_on_the_next_statement",
    edits: [{
      file: SHARING,
      before: `SELECT {identity} AS identity FROM {}.{} a, {}, {}
             WHERE a.{} = {} AND a.{} = {} AND a.{} IS NULL AND {} =`,
      after: `SELECT {identity} AS identity FROM {}.{} a, {}, {}
             WHERE a.{} = {} AND a.{} = {} AND (a.{} IS NULL OR TRUE) AND {} =`,
    }],
    assertions: [{
      anchor: `assert_eq!(
                    row_ids(&body["result"]),
                    expected_ids(&[h], entity),
                    "revoked {end:?}/{entity}/tx={transaction}; self .own survives: {body}"
                );`,
      marker: /revoked Some\("1900-01-01"\)\/person\/tx=false; self \.own survives:/,
    }],
  },
  {
    id: "omit-exact-shared-key",
    description: "Remove the public resolver's exact entity.read.shared permission check.",
    test: "assignment_access_test::resolvers_require_the_exact_target_shared_permission_and_bound_app_identity",
    edits: [{
      file: SHARING,
      before: '"(SELECT rootcx_system.check_access({read_key})) AND',
      after: '"({read_key} IS NOT NULL) AND',
    }],
    assertions: [{
      anchor: `assert!(
                row_ids(&body["result"]).is_empty(),
                "{permission}/tx={transaction}: {body}"
            );`,
      marker: /app:assignment_care:profile\.read\.shared\/tx=false:/,
    }],
  },
  {
    id: "shared-read-authorizes-mutations",
    description: "Let the shared read permission authorize INSERT, UPDATE and DELETE.",
    test: "assignment_access_test::shared_reads_cannot_mutate_assignments_or_reparent_identity_chains",
    edits: [{
      file: POLICIES,
      before: "let predicate = collection_gate(&key, mine.as_deref());",
      after: `let predicate = collection_gate(&key, mine.as_deref());
        let predicate = if action != "read" {
            format!("({predicate}) OR {}", gate(&format!("app:{schema}:{table}.read.shared")))
        } else { predicate };`,
    }],
    assertions: [{
      anchor: `assert!(
                body["error"].as_str().is_some_and(|s| !s.is_empty())
                    || (body["result"]["rows"] == json!([]) && body["result"]["rowCount"] == 0),
                "{label}/tx={transaction} must error or affect zero rows: {body}"
            );`,
      marker: /\/tx=false must error or affect zero rows:/,
    }, {
      anchor: `assert_eq!(
                snapshot(&f.rt).await,
                before,
                "{label}/tx={transaction} changed persisted data"
            );`,
      marker: /\/tx=false changed persisted data/,
    }],
  },
  {
    id: "repeatable-read-callback",
    description: "Pin app transactions to REPEATABLE READ so committed revocation stays invisible.",
    test: "assignment_access_test::concurrent_committed_revocation_is_visible_inside_the_same_bun_callback",
    edits: [{
      file: SQL_PROXY,
      before: "SET LOCAL transaction_isolation = 'read committed';",
      after: "SET LOCAL transaction_isolation = 'repeatable read';",
    }],
    assertions: [{
      anchor: `assert_eq!(
            row_ids(&body["after"]),
            expected_ids(&[h], "note"),
            "{end:?}: {body}"
        );`,
      marker: /Some\("1900-01-01"\):/,
    }],
  },
  {
    id: "table-select-exposes-sensitive-columns",
    description: "Grant table-level SELECT even when sensitive columns require column grants.",
    test: "sensitive_sql_test::worker_sql_denies_sensitive_references_even_for_admins",
    edits: [{
      file: POLICIES,
      before: "if sensitive.is_empty() {",
      after: "if true {",
    }],
    assertions: [{
      anchor: `assert!(
                body["error"]
                    .as_str()
                    .is_some_and(|e| e.contains("permission denied")),
                "{caller}/{sql}: expected PostgreSQL column denial, got {body}"
            );`,
      marker: /user\/SELECT secret FROM vault\.records: expected PostgreSQL column denial/,
    }],
  },
  {
    id: "omit-manifest-sql-admission",
    description: "Omit manifest SQL admission before raw expressions can cause DDL.",
    test: "manifest_sql_admission_test::raw_manifest_sql_is_refused_before_creating_a_schema",
    edits: [{
      file: MANIFEST,
      before: "    crate::governance::row_access::validate_sql_declarations(manifest)",
      after: "    Ok(()) // Mutation: manifest SQL admission omitted.",
    }],
    assertions: [{
      anchor: 'assert_eq!(status, StatusCode::BAD_REQUEST, "{name}: {body}");',
      marker: /check:[\s\S]*\bleft: 20[01]\b/,
    }, {
      anchor: 'assert!(!exists, "{name}: refusal must precede CREATE SCHEMA");',
      marker: /check: refusal must precede CREATE SCHEMA/,
    }],
  },
  {
    id: "skip-versioned-projection",
    description: "Ignore the durable projection and fall back to the valid stored manifest.",
    test: "row_access_projection_test::bootstrap_rejects_malformed_versioned_projection_without_manifest_fallback",
    edits: [{
      file: ROW_ACCESS,
      before: "if let Some(version) = version {",
      after: "if let Some(version) = None::<i32> {",
    }],
    assertions: [{
      anchor: '.expect_err("invalid durable projection must visibly fail bootstrap");',
      marker: /invalid durable projection must visibly fail bootstrap: \(\)/,
    }],
  },
  {
    id: "ignore-missing-owner-column",
    description: "Silently omit ownership policies when the physical owner column is missing.",
    test: "row_access_projection_test::bootstrap_rejects_physical_owner_drift_and_rolls_back_earlier_tables",
    edits: [{
      file: POLICIES,
      before: `return Err(RuntimeError::Invalid(format!("governance: owner column '{schema}.{root}.{root_column}' is missing")));`,
      after: "return Ok(None);",
    }],
    assertions: [{
      anchor: '.expect_err("physical owner drift must visibly fail bootstrap");',
      marker: /physical owner drift must visibly fail bootstrap: \(\)/,
    }],
  },
  {
    id: "regroup-historical-no-share-policy",
    description: "Add OR FALSE to no-share predicates, violating the exact historical policy contract.",
    test: "row_access_projection_test::no_share_policies_match_literal_historical_sql_across_install_redeploy_and_boot",
    edits: [{
      file: POLICIES,
      before: "let predicate = collection_gate(&key, mine.as_deref());",
      after: `let predicate = collection_gate(&key, mine.as_deref());
        let predicate = if shared.is_none() {
            format!("({predicate}) OR FALSE")
        } else { predicate };`,
    }],
    assertions: [{
      anchor: `assert_eq!(
            actual, expected,`,
      marker: /exact pg_policies expressions, commands, roles and permissiveness\s+must match the historical no-share generator/,
    }],
  },
  {
    id: "linear-shared-membership-array",
    description: "Replace hashed shared IN membership with a linear ANY ARRAY over the resolved keys.",
    test: "assignment_access_test::selective_shared_reads_have_real_rls_plans_over_two_hundred_thousand_rows",
    edits: [{
      file: SHARING,
      before: 'Ok((format!("{} IN (SELECT {signature})", quote_ident(&column)), resolver))',
      after: 'Ok((format!("{} = ANY (ARRAY(SELECT {signature}))", quote_ident(&column)), resolver))',
    }],
    assertions: [{
      anchor: `assert!(
            scans.iter().any(|n| n["Filter"].as_str().is_some_and(|f| f.contains("hashed SubPlan"))),
            "shared membership must hash the resolved set once: {plan:#}"
        );`,
      marker: /shared membership must hash the resolved set once:/,
    }],
  },
  {
    id: "remove-both-app-boundaries",
    description: "Remove BOTH the check_access app guard and the explicit shared-resolver app guard.",
    test: "assignment_access_test::resolvers_require_the_exact_target_shared_permission_and_bound_app_identity",
    edits: [{
      file: BOOTSTRAP,
      before: `                 IF left(p_required, 4) = 'app:' AND
                    coalesce(current_setting('rootcx.human_data_request', true), '') <> '1' AND
                    split_part(p_required, ':', 2) IS DISTINCT FROM
                        nullif(current_setting('rootcx.app_id', true), '') THEN
                     RETURN FALSE;
                 END IF;`,
      after: "                 -- Mutation: check_access app boundary omitted.",
    }, {
      file: SHARING,
      before: `(current_setting('rootcx.human_data_request', true) = '1' OR
          nullif(current_setting('rootcx.app_id', true), '') = {})`,
      after: "(TRUE OR {} IS NOT NULL)",
    }],
    assertions: [{
      anchor: `assert!(
                row_ids(&body["result"]).is_empty(),
                "foreign worker/{entity}/tx={transaction}: {body}"
            );`,
      marker: /foreign worker\/person\/tx=false:/,
    }],
  },
];

function sha256(bytes: string | Buffer): string {
  return createHash("sha256").update(bytes).digest("hex");
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function uniqueOffset(source: string, anchor: string, label: string): number {
  const offset = source.indexOf(anchor);
  if (offset < 0 || source.indexOf(anchor, offset + 1) >= 0) {
    throw new Error(`${label}: expected exactly one source anchor; source drift requires review`);
  }
  return offset;
}

function escaped(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function checkSandbox(): void {
  if (process.env.ROOTCX_MUTATION_SANDBOX !== "1") {
    throw new Error("Refusing source mutations without ROOTCX_MUTATION_SANDBOX=1");
  }
  if (process.platform !== "linux" || !existsSync("/.dockerenv")) {
    throw new Error("Run only in a disposable Docker test container");
  }
  if (process.cwd() !== ROOT || realpathSync(ROOT) !== ROOT) {
    throw new Error("The working directory must be the real /src copied into the image");
  }
  const mounts = readFileSync("/proc/self/mountinfo", "utf8").trim().split("\n").map((line) => {
    const parts = line.split(" ");
    const separator = parts.indexOf("-");
    if (parts.length < 10 || separator < 6) throw new Error("Cannot verify mount isolation");
    return {
      path: parts[4].replace(/\\([0-7]{3})/g, (_, octal) => String.fromCharCode(parseInt(octal, 8))),
      filesystem: parts[separator + 1],
    };
  });
  if (!mounts.some((mount) => mount.path === "/" && mount.filesystem === "overlay")) {
    throw new Error("Require a Docker overlay root; do not use a mounted checkout as the root filesystem");
  }
  for (const mount of mounts) {
    if (mount.path === ROOT || mount.path.startsWith(`${ROOT}/`)) {
      throw new Error(`Refusing mount ${mount.path}: /src and its descendants must be image copies`);
    }
  }
  let database: URL;
  try {
    database = new URL(process.env.TEST_DATABASE_URL ?? "");
  } catch {
    throw new Error("TEST_DATABASE_URL must be a valid isolated test database URL");
  }
  if (!["postgres:", "postgresql:"].includes(database.protocol) || database.pathname !== "/rootcx_test") {
    throw new Error("TEST_DATABASE_URL must point to the isolated rootcx_test database");
  }
}

function sourcePath(relative: string): string {
  const path = join(ROOT, relative);
  if (!path.startsWith(`${ROOT}/`) || realpathSync(path) !== path) {
    throw new Error(`Refusing source path outside the copied tree or through a symlink: ${relative}`);
  }
  const stat = lstatSync(path);
  if (!stat.isFile() || stat.nlink !== 1) throw new Error(`Require an ordinary, unlinked source file: ${relative}`);
  return path;
}

function targetOf(mutant: Mutant): TestTarget {
  return mutant.target ?? "governance_test";
}

function evidenceFile(mutant: Mutant): string {
  const target = targetOf(mutant);
  const parts = mutant.test.split("::");
  if (target !== "governance_test") {
    if (parts.length !== 1 || !TARGET_SOURCES[target]) {
      throw new Error(`${mutant.id}: unsupported standalone test mapping`);
    }
    return TARGET_SOURCES[target];
  }
  const file = GOVERNANCE_SOURCES[parts[0]];
  if (parts.length !== 2 || !file) {
    throw new Error(`${mutant.id}: unsupported governance test mapping`);
  }
  return file;
}

function resolveEvidence(mutant: Mutant, sources: Map<string, Buffer>): Evidence[] {
  const test = mutant.test.split("::").at(-1)!;
  const file = evidenceFile(mutant);
  const source = sources.get(file)!.toString("utf8");
  const start = uniqueOffset(source, `async fn ${test}()`, mutant.test);
  const end = source.indexOf("#[tokio::test]", start);
  const body = source.slice(start, end < 0 ? undefined : end);
  return mutant.assertions.map(({ anchor, marker }) => {
    const offset = start + uniqueOffset(body, anchor, `${mutant.id} assertion`);
    return {
      file,
      line: source.slice(0, offset).split("\n").length,
      marker,
      equality: anchor.startsWith("assert_eq!"),
    };
  });
}

function renderMutant(mutant: Mutant, sources: Map<string, Buffer>): Map<string, string> {
  const changed = new Map<string, string>();
  for (const edit of mutant.edits) {
    const source = changed.get(edit.file) ?? sources.get(edit.file)!.toString("utf8");
    const offset = uniqueOffset(source, edit.before, `${mutant.id}/${edit.file}`);
    if (edit.before === edit.after) throw new Error(`${mutant.id}: no-op mutation`);
    changed.set(edit.file, source.slice(0, offset) + edit.after + source.slice(offset + edit.before.length));
  }
  return changed;
}

let interrupted: string | undefined;
let activeChild: ChildProcess | undefined;
let signalEscalation: ReturnType<typeof setTimeout> | undefined;

function killGroup(child: ChildProcess, signal: NodeJS.Signals): void {
  if (child.pid === undefined) return;
  try {
    process.kill(-child.pid, signal);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ESRCH") throw error;
  }
}

function onSignal(signal: string): void {
  interrupted = signal;
  if (activeChild) {
    const child = activeChild;
    killGroup(child, "SIGTERM");
    signalEscalation ??= setTimeout(() => killGroup(child, "SIGKILL"), 2_000);
  }
}
process.on("SIGINT", () => onSignal("SIGINT"));
process.on("SIGTERM", () => onSignal("SIGTERM"));

async function runPhase(command: string[], log: string, timeoutMs: number): Promise<Phase> {
  if (interrupted) throw new Error(`Interrupted by ${interrupted}`);
  process.stderr.write(`[row-access-mutations] ${command.join(" ")}\n`);
  const start = Date.now();
  const fd = openSync(log, "wx", 0o600);
  let child: ChildProcess | undefined;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let escalation: ReturnType<typeof setTimeout> | undefined;
  let timedOut = false;
  try {
    child = spawn(command[0], command.slice(1), {
      cwd: ROOT,
      env: { ...process.env, CARGO_TERM_COLOR: "never", RUST_BACKTRACE: "0" },
      detached: true,
      stdio: ["ignore", fd, fd],
    });
    activeChild = child;
    const completed = new Promise<{ exitCode: number | null; signal: string | null; error?: string }>((resolve) => {
      child!.once("error", (error) => resolve({ exitCode: null, signal: null, error: errorText(error) }));
      child!.once("close", (code, signal) => resolve({ exitCode: code, signal }));
    });
    timer = setTimeout(() => {
      timedOut = true;
      killGroup(child!, "SIGTERM");
      escalation = setTimeout(() => killGroup(child!, "SIGKILL"), 2_000);
    }, timeoutMs);
    const result = await completed;
    return {
      command, log, elapsedMs: Date.now() - start,
      ...result, timedOut, interrupted: interrupted !== undefined,
    };
  } finally {
    if (timer) clearTimeout(timer);
    if (escalation) clearTimeout(escalation);
    if (signalEscalation) clearTimeout(signalEscalation);
    signalEscalation = undefined;
    // Cargo may exit before its tests/workers. Stop the entire owned process
    // group before restoration or the next database-resetting test.
    if (child) killGroup(child, "SIGKILL");
    activeChild = undefined;
    closeSync(fd);
  }
}

function readLog(phase: Phase): string {
  if (statSync(phase.log).size > MAX_LOG_BYTES) throw new Error(`Log exceeds classification limit: ${phase.log}`);
  return readFileSync(phase.log, "utf8");
}

function phaseProblem(phase: Phase): Outcome | undefined {
  if (phase.interrupted) return "interrupted";
  if (phase.timedOut) return "timeout";
  if (phase.error || phase.signal || phase.exitCode === null) return "unclassified_error";
  return undefined;
}

function passedOne(phase: Phase): boolean {
  return !phaseProblem(phase) && phase.exitCode === 0
    && /test result: ok\. 1 passed; 0 failed; 0 ignored;/.test(readLog(phase));
}

function classify(result: Result, phase: Phase, intended: Evidence[]): void {
  const problem = phaseProblem(phase);
  if (problem) {
    result.outcome = problem;
    return;
  }
  const output = readLog(phase);
  if (phase.exitCode === 0) {
    result.outcome = passedOne(phase) ? "survived" : "unclassified_error";
    return;
  }
  // Both the harness summary and the panic must identify the intended test.
  // Exit 101 alone also means compilation failure and is never evidence.
  if (phase.exitCode !== 101
    || !/test result: FAILED\. 0 passed; 1 failed; 0 ignored;/.test(output)
    || !new RegExp(`^\\s+${escaped(result.test)}\\s*$`, "m").test(output)) {
    result.outcome = "unclassified_error";
    result.error = output.slice(-8_000);
    return;
  }
  const panics = [...output.matchAll(
    /thread '([^']+)'(?: \(\d+\))? panicked at ([^\n]+):(\d+):(\d+):\r?\n([\s\S]*?)(?=\nthread '|\nfailures:|$)/g,
  )];
  // An extra task panic can indicate infrastructure trouble even if an
  // assertion subsequently fails. Fail closed instead of claiming a kill.
  if (panics.length === 1 && panics[0][1] === result.test) {
    const [, , file, line, , message] = panics[0];
    const matched = intended.find((evidence) =>
      (file === evidence.file || file.endsWith(`/${evidence.file}`))
      && Number(line) === evidence.line
      && evidence.marker.test(message)
      && (!evidence.equality || /assertion `left == right` failed/.test(message)));
    if (matched) {
      result.outcome = "killed";
      result.evidence = { file, line: Number(line), message: message.slice(0, 4_000).trim() };
      return;
    }
  }
  result.outcome = /boot failed|fixture worker deploy|did not become healthy|register harness admin/.test(
    panics.map((panic) => panic[5]).join("\n"),
  ) ? "boot_error" : "unclassified_error";
  result.error = output.slice(-8_000);
}

function verifySources(sources: Map<string, Buffer>): void {
  for (const [file, original] of sources) {
    if (!readFileSync(sourcePath(file)).equals(original)) {
      throw new Error(`Source changed outside this mutant: ${file}`);
    }
  }
}

const report = {
  schemaVersion: 1,
  startedAt: new Date().toISOString(),
  finishedAt: "",
  status: "error" as "passed" | "failed" | "error",
  sandbox: { verified: false, root: ROOT, noSourceMounts: false },
  runtime: { bun: Bun.version, platform: process.platform, arch: process.arch, rustc: "", cargo: "" },
  limits: { compileMs: COMPILE_MS, testMs: TEST_MS },
  directory: "",
  targets: [] as TestTarget[],
  sources: {} as Record<string, string>,
  baseline: { clean: undefined as Phase | undefined, compile: undefined as Phase | undefined, tests: [] as { target: TestTarget; test: string; phase: Phase; passed: boolean }[] },
  mutants: [] as Result[],
  errors: [] as string[],
  allSourcesRestored: false,
};

async function main(): Promise<void> {
  let lock: number | undefined;
  const sources = new Map<string, Buffer>();
  try {
    checkSandbox();
    process.env.CARGO_TARGET_DIR = `${process.env.CARGO_TARGET_DIR ?? "/cache/target"}-row-access-mutations`;
    report.sandbox = { verified: true, root: ROOT, noSourceMounts: true };
    lock = openSync(LOCK, "wx", 0o600);
    writeFileSync(lock, `${process.pid}\n`);
    report.directory = mkdtempSync("/tmp/row-access-mutations-");
    process.stderr.write(`[row-access-mutations] logs and report: ${report.directory}\n`);
    const requested = process.argv.slice(2);
    for (const id of requested) {
      if (!mutants.some((mutant) => mutant.id === id)) throw new Error(`Unknown mutant: ${id}`);
    }
    const selected = mutants.filter((mutant) => requested.length === 0 || requested.includes(mutant.id));
    const targets = [...new Set(selected.map(targetOf))];
    report.targets = targets;
    const files = new Set([
      "Cargo.lock",
      "docker/Dockerfile.test",
      "scripts/row-access-mutations.ts",
      "core/tests/harness/mod.rs",
      ...targets.map((target) => TARGET_SOURCES[target]),
      ...selected.flatMap((mutant) => mutant.edits.map((edit) => edit.file)),
      ...selected.map(evidenceFile),
    ]);
    for (const file of files) {
      const bytes = readFileSync(sourcePath(file));
      sources.set(file, bytes);
      report.sources[file] = sha256(bytes);
    }
    // Resolve ALL patches and intended assertion locations before compilation
    // or writes. Whitespace/source changes require explicit anchor maintenance.
    const prepared = selected.map((mutant) => {
      const changed = renderMutant(mutant, sources);
      const evidence = resolveEvidence(mutant, sources);
      const result: Result = {
        id: mutant.id, description: mutant.description, target: targetOf(mutant),
        test: mutant.test, outcome: "not_run",
        patches: mutant.edits,
        edits: [...changed].map(([file, after]) => ({
          file, beforeSha256: sha256(sources.get(file)!), afterSha256: sha256(after),
        })),
        intendedAssertions: evidence.map(({ file, line, marker }) => ({ file, line, marker: marker.source })),
      };
      report.mutants.push(result);
      return { mutant, changed, evidence, result };
    });
    for (const tool of ["rustc", "cargo"] as const) {
      const phase = await runPhase([tool, "--version"], join(report.directory, `${tool}.log`), 10_000);
      if (phaseProblem(phase) || phase.exitCode !== 0) throw new Error(`Cannot identify ${tool}: ${phase.log}`);
      report.runtime[tool] = readLog(phase).trim();
    }
    report.baseline.clean = await runPhase(
      ["cargo", "clean", "--locked", "-p", "rootcx-core"],
      join(report.directory, "baseline-clean.log"), COMPILE_MS,
    );
    if (phaseProblem(report.baseline.clean) || report.baseline.clean.exitCode !== 0) {
      throw new Error(`Baseline cache cleanup failed:\n${readLog(report.baseline.clean).slice(-8_000)}`);
    }
    report.baseline.compile = await runPhase(
      [...CARGO, ...targets.flatMap((target) => ["--test", target]), "--no-run"],
      join(report.directory, `baseline-compile-${targets.join("-")}.log`), COMPILE_MS,
    );
    if (phaseProblem(report.baseline.compile) || report.baseline.compile.exitCode !== 0) {
      throw new Error(`Baseline compile failed; no mutations applied:\n${readLog(report.baseline.compile).slice(-8_000)}`);
    }
    const baselineTests = new Map(selected.map((mutant) => [
      `${targetOf(mutant)}::${mutant.test}`, { target: targetOf(mutant), test: mutant.test },
    ]));
    for (const { target, test } of baselineTests.values()) {
      const phase = await runPhase(
        [...CARGO, "--test", target, test, "--", "--nocapture", "--test-threads=1"],
        join(report.directory, `baseline-${target}-${test.replaceAll("::", "-")}.log`), TEST_MS,
      );
      const passed = passedOne(phase);
      report.baseline.tests.push({ target, test, phase, passed });
      if (!passed) throw new Error(`Baseline must pass exactly one test: ${target}/${test}; no mutations applied:\n${readLog(phase).slice(-8_000)}`);
    }
    for (const { mutant, changed, evidence, result } of prepared) {
      verifySources(sources);
      if (interrupted) throw new Error(`Interrupted by ${interrupted}`);
      try {
        for (const [file, content] of changed) writeFileSync(sourcePath(file), content);
        result.compile = await runPhase(
          [...CARGO, "--test", result.target, mutant.test, "--no-run"],
          join(report.directory, `${result.target}-${mutant.id}-compile.log`), COMPILE_MS,
        );
        const problem = phaseProblem(result.compile);
        if (problem || result.compile.exitCode !== 0) {
          result.outcome = problem ?? "compile_error";
          result.error = readLog(result.compile).slice(-8_000);
        } else {
          result.execution = await runPhase(
            [...CARGO, "--test", result.target, mutant.test, "--", "--nocapture", "--test-threads=1"],
            join(report.directory, `${result.target}-${mutant.id}-test.log`), TEST_MS,
          );
          classify(result, result.execution, evidence);
        }
      } catch (error) {
        result.outcome = interrupted ? "interrupted" : "unclassified_error";
        result.error = errorText(error);
      } finally {
        // Try every file even if restoring one fails; never leave the second
        // half of the compound app-boundary mutation behind by short circuit.
        const errors: string[] = [];
        for (const file of changed.keys()) {
          try {
            writeFileSync(sourcePath(file), sources.get(file)!);
          } catch (error) {
            errors.push(`${file}: ${errorText(error)}`);
          }
        }
        try { verifySources(sources); } catch (error) { errors.push(errorText(error)); }
        result.restored = errors.length === 0;
        if (errors.length) {
          result.outcome = "restore_error";
          result.error = errors.join("; ");
        }
      }
      process.stderr.write(`[row-access-mutations] ${result.target}/${mutant.id}: ${result.outcome}\n`);
      if (!result.restored) throw new Error("Restoration failed; discard this container");
      if (interrupted) throw new Error(`Interrupted by ${interrupted}`);
    }
    report.status = report.mutants.every((result) => result.outcome === "killed") ? "passed" : "failed";
  } catch (error) {
    report.errors.push(errorText(error));
    report.status = "error";
  } finally {
    if (sources.size) {
      try {
        verifySources(sources);
        report.allSourcesRestored = true;
      } catch (error) {
        report.errors.push(errorText(error));
        report.status = "error";
      }
    }
    if (lock !== undefined) {
      try {
        closeSync(lock);
        unlinkSync(LOCK);
      } catch (error) {
        report.errors.push(`Cannot release runner lock: ${errorText(error)}`);
        report.status = "error";
      }
    }
    report.finishedAt = new Date().toISOString();
    const json = JSON.stringify(report, null, 2) + "\n";
    if (report.directory) {
      try { writeFileSync(join(report.directory, "report.json"), json, { mode: 0o600 }); }
      catch (error) {
        report.errors.push(`Cannot write report: ${errorText(error)}`);
        report.status = "error";
      }
    }
    process.stdout.write(JSON.stringify(report, null, 2) + "\n");
    process.exitCode = report.status === "passed" && report.allSourcesRestored ? 0 : 1;
  }
}

await main();
