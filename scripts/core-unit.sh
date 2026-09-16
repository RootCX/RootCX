#!/usr/bin/env bash
# Always use a fresh database, including when TEST_DATABASE_URL is inherited.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

container=""
cleanup() {
  if [[ -n "$container" ]]; then
    docker rm -f -v "$container" >/dev/null
  fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

container=$(docker create \
  --user root --entrypoint /pg-entrypoint.sh \
  -e POSTGRES_USER=rootcx \
  -e POSTGRES_PASSWORD=rootcx \
  -e POSTGRES_DB=rootcx \
  -e PGDATA=/tmp/pgdata \
  -p 127.0.0.1::5432 \
  ghcr.io/rootcx/postgresql:16-pgmq-cron)
docker start "$container" >/dev/null

# Probe the container's network interface: initdb's temporary server listens
# only on localhost and must not be mistaken for the final server.
ready=false
for ((attempt = 0; attempt < 120; attempt++)); do
  if docker exec -e PGPASSWORD=rootcx "$container" sh -c \
    'psql -h "$(hostname -i)" -U rootcx -d rootcx -Atqc "SELECT 1"' \
    >/dev/null 2>&1; then
    ready=true
    break
  fi
  if [[ "$(docker inspect -f '{{.State.Running}}' "$container")" != true ]]; then
    break
  fi
  sleep 0.5
done
if [[ "$ready" != true ]]; then
  echo "Disposable PostgreSQL failed to become ready" >&2
  docker logs "$container" >&2
  exit 1
fi

port=$(docker inspect -f '{{(index (index .NetworkSettings.Ports "5432/tcp") 0).HostPort}}' "$container")
[[ "$port" =~ ^[0-9]+$ ]] || { echo "Missing Docker PostgreSQL port" >&2; exit 1; }
export TEST_DATABASE_URL="postgres://rootcx:rootcx@127.0.0.1:${port}/rootcx"
echo "[core/unit] disposable PostgreSQL listening on 127.0.0.1:${port}"
cargo test -p rootcx-core --lib "$@" -- --nocapture
