#!/bin/sh
set -e

# Hide /proc entries from other UIDs (workers spawned as UID 1001 cannot read /proc/1/environ)
if mount -o remount,hidepid=2 /proc 2>/dev/null; then
    echo "proc remounted with hidepid=2"
fi

exec "$@"
