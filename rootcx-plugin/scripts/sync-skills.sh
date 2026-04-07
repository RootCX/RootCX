#!/usr/bin/env bash
# Sync canonical skills from .agents/skills/ into the plugin's skills/ dir.
# Run from the rootCX2 repo root.
set -euo pipefail
SRC="$(dirname "$0")/../../.agents/skills"
DST="$(dirname "$0")/../skills"
rm -rf "$DST"
mkdir -p "$DST"
for skill in rootcx-manifest rootcx-sdk-hooks rootcx-ui rootcx-backend-worker rootcx-rest-api rootcx-agent; do
  cp -r "$SRC/$skill" "$DST/"
done
echo "✓ synced 6 skills to $DST"
