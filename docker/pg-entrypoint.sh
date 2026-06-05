#!/bin/bash
set -e

PG=/usr/lib/postgresql/16/bin
PGDATA="${PGDATA:-/data/pgdata}"
PG_UID="$(id -u postgres)"
PG_GID="$(id -g postgres)"

chown "$PG_UID:$PG_GID" "$(dirname "$PGDATA")"
mkdir -p "$PGDATA"
chown "$PG_UID:$PG_GID" "$PGDATA"

if [ ! -s "$PGDATA/PG_VERSION" ]; then
  # Bootstrap with local trust ONLY so the password can be set without a
  # chicken-and-egg. The final pg_hba below is scram-sha-256 everywhere.
  su postgres -c "$PG/initdb -D $PGDATA --username=${POSTGRES_USER:-postgres} --auth-local=trust --auth-host=scram-sha-256"
  cat >> "$PGDATA/postgresql.conf" <<'PGCONF'
listen_addresses='*'
shared_preload_libraries='pg_cron'
cron.use_background_workers=on
password_encryption='scram-sha-256'
PGCONF
  echo "cron.database_name='${POSTGRES_DB:-postgres}'" >> "$PGDATA/postgresql.conf"

  su postgres -c "$PG/pg_ctl -D $PGDATA start -w -o '-c listen_addresses=localhost'"
  su postgres -c "$PG/createdb -U ${POSTGRES_USER:-postgres} ${POSTGRES_DB:-postgres}" 2>/dev/null || true
  su postgres -c "$PG/psql -U ${POSTGRES_USER:-postgres} -d postgres -c \"ALTER USER \\\"${POSTGRES_USER:-postgres}\\\" PASSWORD '${POSTGRES_PASSWORD:-postgres}';\""
  su postgres -c "$PG/pg_ctl -D $PGDATA stop -w"

  # Lock down pg_hba: scram-sha-256 everywhere, ZERO trust lines. A worker
  # that finds the socket or port cannot connect without the password. pg_cron
  # uses background workers (internal connections, no pg_hba), so this is safe.
  cat > "$PGDATA/pg_hba.conf" <<'PGHBA'
local   all   all                  scram-sha-256
host    all   all   127.0.0.1/32   scram-sha-256
host    all   all   ::1/128        scram-sha-256
host    all   all   0.0.0.0/0      scram-sha-256
PGHBA
  chown "$PG_UID:$PG_GID" "$PGDATA/pg_hba.conf"
fi

exec su postgres -c "exec $PG/postgres -D $PGDATA"
