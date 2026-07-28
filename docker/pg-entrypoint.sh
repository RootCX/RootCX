#!/bin/bash
set -e

PG=/usr/lib/postgresql/16/bin
PGDATA="${PGDATA:-/data/pgdata}"
export PGHOST=/tmp

mkdir -p "$PGDATA"
# Some CSI drivers relax directory permissions while remounting a retained
# volume. PostgreSQL refuses to start unless PGDATA is 0700 or 0750.
chmod 0700 "$PGDATA"

# OpenShift assigns an arbitrary UID that does not exist in /etc/passwd.
# PostgreSQL requires a resolvable effective user during initdb.
if ! id -un >/dev/null 2>&1; then
  export NSS_WRAPPER_PASSWD=/tmp/passwd
  export NSS_WRAPPER_GROUP=/etc/group
  cp /etc/passwd "$NSS_WRAPPER_PASSWD"
  echo "rootcx:x:$(id -u):$(id -g):RootCX PostgreSQL:$PGDATA:/sbin/nologin" >> "$NSS_WRAPPER_PASSWD"
  export LD_PRELOAD=libnss_wrapper.so
fi

if [ ! -s "$PGDATA/PG_VERSION" ]; then
  # Bootstrap with local trust ONLY so the password can be set without a
  # chicken-and-egg. The final pg_hba below is scram-sha-256 everywhere.
  "$PG/initdb" -D "$PGDATA" --username="${POSTGRES_USER:-postgres}" --auth-local=trust --auth-host=scram-sha-256
  cat >> "$PGDATA/postgresql.conf" <<'PGCONF'
listen_addresses='*'
unix_socket_directories='/tmp'
shared_preload_libraries='pg_cron'
cron.use_background_workers=on
password_encryption='scram-sha-256'
PGCONF
  echo "cron.database_name='${POSTGRES_DB:-postgres}'" >> "$PGDATA/postgresql.conf"

  "$PG/pg_ctl" -D "$PGDATA" start -w -o "-c listen_addresses=localhost"
  "$PG/createdb" -U "${POSTGRES_USER:-postgres}" "${POSTGRES_DB:-postgres}" 2>/dev/null || true
  if [ -n "${POSTGRES_EXTRA_DATABASES:-}" ]; then
    old_ifs=$IFS
    IFS=','
    for database in $POSTGRES_EXTRA_DATABASES; do
      "$PG/createdb" -U "${POSTGRES_USER:-postgres}" "$database" 2>/dev/null || true
    done
    IFS=$old_ifs
  fi
  "$PG/psql" -U "${POSTGRES_USER:-postgres}" -d postgres \
    --set=username="${POSTGRES_USER:-postgres}" \
    --set=password="${POSTGRES_PASSWORD:-postgres}" <<'SQL'
SELECT format('ALTER USER %I PASSWORD %L', :'username', :'password') \gexec
SQL
  "$PG/pg_ctl" -D "$PGDATA" stop -w

  # Lock down pg_hba: scram-sha-256 everywhere, ZERO trust lines. A worker
  # that finds the socket or port cannot connect without the password. pg_cron
  # uses background workers (internal connections, no pg_hba), so this is safe.
  cat > "$PGDATA/pg_hba.conf" <<'PGHBA'
local   all   all                  scram-sha-256
host    all   all   127.0.0.1/32   scram-sha-256
host    all   all   ::1/128        scram-sha-256
host    all   all   0.0.0.0/0      scram-sha-256
PGHBA
fi

exec "$PG/postgres" -D "$PGDATA"
