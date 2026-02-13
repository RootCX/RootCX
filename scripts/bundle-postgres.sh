#!/usr/bin/env bash
#
# bundle-postgres.sh
#
# Copies PostgreSQL binaries + dylib dependencies + share data from Homebrew
# into the Tauri sidecar/resource structure, making the app self-contained.
#
# Layout mirrors PostgreSQL's expected directory structure so that the
# compiled-in relative path resolution works natively:
#
#   src-tauri/
#   +-- bin/                    PG binaries (sidecars)
#   +-- lib/                    PG dylibs
#   +-- share/postgresql/       PG share data (timezone, extensions, etc.)
#
# PostgreSQL resolves: <bindir>/../share/postgresql  -> share/postgresql  OK
# PostgreSQL resolves: <bindir>/../lib               -> lib               OK
#
# Usage:
#   ./scripts/bundle-postgres.sh
#
# Requires: Homebrew PostgreSQL installed (brew install postgresql@17)
#
set -euo pipefail

# -- Configuration -----------------------------------------------------------

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

PG_PREFIX="/opt/homebrew/opt/postgresql@17"

ARCH="$(uname -m)"
case "$ARCH" in
  arm64)  TARGET_TRIPLE="aarch64-apple-darwin" ;;
  x86_64) TARGET_TRIPLE="x86_64-apple-darwin" ;;
  *)      echo "Unsupported architecture: $ARCH"; exit 1 ;;
esac

SRC_TAURI="$ROOT_DIR/apps/studio-desktop/src-tauri"
BIN_DIR="$SRC_TAURI/bin"
LIB_DIR="$SRC_TAURI/lib"
SHARE_DIR="$SRC_TAURI/share/postgresql"

PG_BINS=(initdb pg_ctl postgres)

# -- Preflight ---------------------------------------------------------------

if [ ! -d "$PG_PREFIX" ]; then
  echo "ERROR: PostgreSQL not found at $PG_PREFIX"
  echo "       Install it:  brew install postgresql@17"
  exit 1
fi

echo "=== RootCX PostgreSQL Bundler ==="
echo "Source:  $PG_PREFIX"
echo "Target:  $TARGET_TRIPLE"
echo ""

# -- 1. Clean previous bundle ------------------------------------------------

rm -rf "$BIN_DIR" "$LIB_DIR" "$SRC_TAURI/share"
mkdir -p "$BIN_DIR" "$LIB_DIR" "$SHARE_DIR"

# -- 2. Copy binaries with Tauri sidecar naming ------------------------------

echo "--- Copying binaries ---"
for bin in "${PG_BINS[@]}"; do
  src="$PG_PREFIX/bin/$bin"
  dst="$BIN_DIR/${bin}-${TARGET_TRIPLE}"
  cp "$src" "$dst"
  chmod +x "$dst"
  echo "  $bin -> ${bin}-${TARGET_TRIPLE}"

  # Symlink without suffix -- initdb spawns "postgres" by name.
  ln -sf "${bin}-${TARGET_TRIPLE}" "$BIN_DIR/$bin"
done

# -- 3. Copy share directory --------------------------------------------------

echo ""
echo "--- Copying share data ---"
cp -R "$PG_PREFIX/share/postgresql/" "$SHARE_DIR/"
echo "  share/postgresql -> share/postgresql"

# -- 4. Collect non-system dylib dependencies recursively ---------------------

echo ""
echo "--- Collecting dylibs ---"

LIB_COUNT=0

# Returns non-system dylib paths referenced by a binary.
# Captures both absolute /opt/homebrew paths AND @loader_path refs.
list_homebrew_deps() {
  local binary="$1"
  local src_dir="$2"

  otool -L "$binary" 2>/dev/null | awk 'NR>1 { print $1 }' | while read -r ref; do
    if [[ "$ref" == /opt/homebrew/* ]]; then
      echo "$ref"
    elif [[ "$ref" == @loader_path/* && -n "$src_dir" ]]; then
      local leaf="${ref#@loader_path/}"
      local resolved="$src_dir/$leaf"
      if [[ -f "$resolved" ]]; then
        echo "$resolved"
      fi
    fi
  done
}

# Recursively copy a dylib and its dependencies.
copy_lib() {
  local src="$1"
  local name
  name="$(basename "$src")"

  if [[ -f "$LIB_DIR/$name" ]]; then return; fi

  local real
  real="$(python3 -c "import os; print(os.path.realpath('$src'))" 2>/dev/null || echo "$src")"

  if [[ ! -f "$real" ]]; then
    echo "  WARN: $src not found, skipping"
    return
  fi

  cp "$real" "$LIB_DIR/$name"
  chmod 644 "$LIB_DIR/$name"
  echo "  $name"
  LIB_COUNT=$((LIB_COUNT + 1))

  local src_dir
  src_dir="$(dirname "$real")"
  for dep in $(list_homebrew_deps "$real" "$src_dir"); do
    copy_lib "$dep"
  done
}

# Seed: collect deps of all three PG binaries from Homebrew originals.
for bin in "${PG_BINS[@]}"; do
  for dep in $(list_homebrew_deps "$PG_PREFIX/bin/$bin" "$PG_PREFIX/bin"); do
    copy_lib "$dep"
  done
done

echo "  ($LIB_COUNT libraries total)"

# -- 5. Rewrite dylib references to use @loader_path -------------------------

echo ""
echo "--- Relinking dylibs ---"

list_non_system_refs() {
  otool -L "$1" 2>/dev/null | awk 'NR>1 { print $1 }' | while read -r ref; do
    if [[ "$ref" == /opt/homebrew/* ]] || [[ "$ref" == @loader_path/* ]]; then
      echo "$ref"
    fi
  done
}

# Rewrite references inside the bundled dylibs (lib -> lib = same dir).
for lib in "$LIB_DIR"/*.dylib; do
  local_name="$(basename "$lib")"
  install_name_tool -id "@loader_path/$local_name" "$lib" 2>/dev/null || true

  for ref in $(list_non_system_refs "$lib"); do
    dep_name="$(basename "$ref")"
    if [[ -f "$LIB_DIR/$dep_name" ]]; then
      install_name_tool -change "$ref" "@loader_path/$dep_name" "$lib" 2>/dev/null || true
    fi
  done
done
echo "  dylib -> dylib: @loader_path/<name>"

# Rewrite references inside the binaries.
# bin/ and lib/ are siblings under src-tauri/, so: ../lib/<name>
for bin in "${PG_BINS[@]}"; do
  binary="$BIN_DIR/${bin}-${TARGET_TRIPLE}"
  for ref in $(list_non_system_refs "$binary"); do
    dep_name="$(basename "$ref")"
    if [[ -f "$LIB_DIR/$dep_name" ]]; then
      install_name_tool -change "$ref" "@loader_path/../lib/$dep_name" "$binary" 2>/dev/null || true
    fi
  done
done
echo "  binary -> dylib: @loader_path/../lib/<name>"

# -- 6. Re-sign after relinking (required on Apple Silicon) -------------------

echo ""
echo "--- Codesigning ---"

for lib in "$LIB_DIR"/*.dylib; do
  codesign --force --sign - "$lib" 2>/dev/null
done
echo "  Re-signed $(ls "$LIB_DIR"/*.dylib | wc -l | tr -d ' ') dylibs"

for bin in "${PG_BINS[@]}"; do
  codesign --force --sign - "$BIN_DIR/${bin}-${TARGET_TRIPLE}" 2>/dev/null
done
echo "  Re-signed ${#PG_BINS[@]} binaries"

# -- 7. Verify ----------------------------------------------------------------

echo ""
echo "--- Verification ---"
ERRORS=0

for bin in "${PG_BINS[@]}"; do
  binary="$BIN_DIR/${bin}-${TARGET_TRIPLE}"
  remaining="$(otool -L "$binary" 2>/dev/null | grep -c '/opt/homebrew' || true)"
  if [[ "$remaining" -gt 0 ]]; then
    echo "  WARN: $bin still has $remaining Homebrew references:"
    otool -L "$binary" | grep '/opt/homebrew' | awk '{print "    " $1}'
    ERRORS=1
  else
    echo "  $bin: OK (no Homebrew references)"
  fi
done

for lib in "$LIB_DIR"/*.dylib; do
  lib_name="$(basename "$lib")"
  for ref in $(otool -L "$lib" 2>/dev/null | awk 'NR>1{print $1}'); do
    if [[ "$ref" == @loader_path/* ]]; then
      leaf="${ref#@loader_path/}"
      if [[ ! -f "$LIB_DIR/$leaf" ]]; then
        echo "  WARN: $lib_name references missing @loader_path/$leaf"
        ERRORS=1
      fi
    fi
  done
done

if [[ "$ERRORS" -eq 0 ]]; then
  echo ""
  echo "=== Bundle complete. Binaries are self-contained. ==="
else
  echo ""
  echo "=== Bundle complete with warnings. DYLD_LIBRARY_PATH fallback will handle remaining refs. ==="
fi

# -- Summary ------------------------------------------------------------------

echo ""
echo "Layout:"
echo "  src-tauri/bin/                  $(ls "$BIN_DIR"/*-* 2>/dev/null | wc -l | tr -d ' ') binaries"
echo "  src-tauri/lib/                  $(ls "$LIB_DIR"/*.dylib 2>/dev/null | wc -l | tr -d ' ') dylibs"
echo "  src-tauri/share/postgresql/     $(du -sh "$SHARE_DIR" 2>/dev/null | awk '{print $1}')"
