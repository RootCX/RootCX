#!/usr/bin/env bash
set -euo pipefail

mode="${1:-verify}"
target="${2:-governance_test}"
filter="${3:-}"
build=(cargo test --locked -p rootcx-core --no-run)
case "$mode" in
  unit) build+=(--lib) ;;
  integration) build+=(--test "$target") ;;
  verify) build+=(--lib --test governance_test --test worker_lifecycle_test --test app_migrations_test) ;;
  *) echo "unknown test mode: $mode" >&2; exit 2 ;;
esac

echo "[build] compiling tests (maximum 20 minutes)"
timeout --kill-after=10s 20m "${build[@]}"

# Keep the build cache owned by the container, but exercise Core and Bun with
# the production image's ordinary UID. Root would hide permission regressions.
if [[ "$(id -u)" == 0 ]]; then
  runner_target="$(rustc -vV | sed -n 's/^host: //p')"
  runner_target="${runner_target//-/_}"
  export "CARGO_TARGET_${runner_target^^}_RUNNER=setpriv --reuid=1000 --regid=1000 --clear-groups"
  echo "[runtime] Core tests execute as UID 1000 without supplementary groups"
fi

if [[ "$mode" != integration ]]; then
  echo "[unit] running library tests (maximum 5 minutes)"
  timeout --kill-after=10s 5m cargo test --locked -p rootcx-core --lib \
    -- "$filter" --test-threads="${TEST_THREADS:-2}"
fi
if [[ "$mode" != unit ]]; then
  echo "[integration] fresh database per test; maximum 2 minutes per test, 20 minutes total"
  targets=(--test "$target")
  if [[ "$mode" == verify ]]; then
    targets+=(--test worker_lifecycle_test --test app_migrations_test)
  fi
  timeout --kill-after=10s 20m cargo nextest run --locked -p rootcx-core \
    --no-fail-fast "${targets[@]}" -- "$filter"
fi
if [[ "$mode" == verify ]]; then
  echo "[worker] running Bun tests (maximum 2 minutes)"
  timeout --kill-after=10s 2m bun test core/src/backend_prelude.test.ts
fi
