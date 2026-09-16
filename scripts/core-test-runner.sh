#!/usr/bin/env bash
set -euo pipefail

mode="${1:-verify}"
target="${2:-governance_test}"
filter="${3:-}"
build=(cargo test --locked -p rootcx-core --no-run)
case "$mode" in
  unit) build+=(--lib) ;;
  integration) build+=(--test "$target") ;;
  verify) build+=(--lib --test governance_test) ;;
  *) echo "unknown test mode: $mode" >&2; exit 2 ;;
esac

echo "[build] compiling tests (maximum 20 minutes)"
timeout --kill-after=10s 20m "${build[@]}"

if [[ "$mode" != integration ]]; then
  echo "[unit] running library tests (maximum 5 minutes)"
  timeout --kill-after=10s 5m cargo test --locked -p rootcx-core --lib \
    -- "$filter" --test-threads="${TEST_THREADS:-2}"
fi
if [[ "$mode" != unit ]]; then
  echo "[integration] fresh database per test; maximum 2 minutes per test, 20 minutes total"
  timeout --kill-after=10s 20m cargo nextest run --locked -p rootcx-core \
    --test "$target" -- "$filter"
fi
if [[ "$mode" == verify ]]; then
  echo "[worker] running Bun tests (maximum 2 minutes)"
  timeout --kill-after=10s 2m bun test core/src/backend_prelude.test.ts
fi
