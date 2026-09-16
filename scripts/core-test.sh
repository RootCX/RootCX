#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

export CARGO_JOBS="${CARGO_JOBS:-2}"
export TEST_THREADS="${TEST_THREADS:-2}"
project="rootcx-test-$(id -u)-$$"
compose=(docker compose -p "$project" -f docker-compose.test.yml)

cleanup() {
  "${compose[@]}" down --timeout 10 --remove-orphans --rmi local >/dev/null
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

docker volume create rootcx-core-test-cache >/dev/null
"${compose[@]}" build tests
"${compose[@]}" up -d --wait --wait-timeout 45 postgres
"${compose[@]}" run --rm -T tests "${1:-verify}" "${2:-governance_test}" "${3:-}"
git diff --check
