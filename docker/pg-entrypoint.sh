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
  su postgres -c "$PG/initdb -D $PGDATA --username=${POSTGRES_USER:-postgres} --auth=trust"
  echo "host all all 0.0.0.0/0 md5" >> "$PGDATA/pg_hba.conf"
  echo "listen_addresses='*'" >> "$PGDATA/postgresql.conf"

  su postgres -c "$PG/pg_ctl -D $PGDATA start -w -o '-c listen_addresses=localhost'"
  su postgres -c "$PG/createdb -U ${POSTGRES_USER:-postgres} ${POSTGRES_DB:-postgres}" 2>/dev/null || true
  su postgres -c "$PG/psql -U ${POSTGRES_USER:-postgres} -d postgres -c \"ALTER USER \\\"${POSTGRES_USER:-postgres}\\\" PASSWORD '${POSTGRES_PASSWORD:-postgres}';\""
  su postgres -c "$PG/pg_ctl -D $PGDATA stop -w"
fi

exec su postgres -c "exec $PG/postgres -D $PGDATA"
