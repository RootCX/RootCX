#!/usr/bin/env bash
# Download platform-specific runtime dependencies (PostgreSQL + Bun) into core/resources/.
#
# Usage: scripts/fetch-deps.sh [TARGET]
# TARGET defaults to the current host triple.
#
# Override versions via env:
#   ROOTCX_PG_VERSION   (default: 18.2.0)
#   ROOTCX_BUN_VERSION  (default: 1.3.10)

set -euo pipefail

TARGET="${1:-$(rustc -vV 2>/dev/null | awk '/^host:/{print $2}')}"
PG_VERSION="${ROOTCX_PG_VERSION:-18.2.0}"
BUN_VERSION="${ROOTCX_BUN_VERSION:-1.3.10}"

RESOURCES="$(dirname "$0")/../core/resources"
mkdir -p "$RESOURCES"

log() { echo "[fetch-deps] $*"; }
die() { echo "[fetch-deps] ERROR: $*" >&2; exit 1; }

fetch() {
    local url="$1" out="$2"
    if command -v curl &>/dev/null; then
        curl -fsSL --retry 3 -o "$out" "$url"
    else
        wget -q --tries=3 -O "$out" "$url" || die "wget failed for $url"
    fi
}

# ── PostgreSQL ────────────────────────────────────────────────────────────────
# Source: https://github.com/theseus-rs/postgresql-binaries

fetch_postgres() {
    local dir="$RESOURCES/postgresql-${PG_VERSION}-${TARGET}"
    if [[ -d "$dir" ]] && [[ -f "$dir/bin/pg_ctl" || -f "$dir/bin/pg_ctl.exe" ]]; then
        log "PostgreSQL ${PG_VERSION} for ${TARGET} already present — skipping."; return
    fi
    local archive="postgresql-${PG_VERSION}-${TARGET}.tar.gz"
    local url="https://github.com/theseus-rs/postgresql-binaries/releases/download/${PG_VERSION}/${archive}"
    local tmp; tmp=$(mktemp -d); trap 'rm -rf "$tmp"' RETURN
    log "Downloading PostgreSQL ${PG_VERSION} for ${TARGET} …"
    fetch "$url" "$tmp/$archive"
    log "Extracting …"
    tar -xzf "$tmp/$archive" -C "$RESOURCES"
    [[ -d "$dir" ]] || die "extraction failed — expected $dir"
    log "PostgreSQL ready at $dir"
}

# ── Bun ───────────────────────────────────────────────────────────────────────
# Source: https://github.com/oven-sh/bun/releases
# Release asset naming: bun-{platform}-{arch}.zip

fetch_bun() {
    local bun_target is_windows=false
    case "$TARGET" in
        aarch64-apple-darwin)      bun_target="bun-darwin-aarch64" ;;
        x86_64-apple-darwin)       bun_target="bun-darwin-x64" ;;
        x86_64-unknown-linux-gnu)  bun_target="bun-linux-x64" ;;
        aarch64-unknown-linux-gnu) bun_target="bun-linux-aarch64" ;;
        x86_64-pc-windows-msvc)    bun_target="bun-windows-x64"; is_windows=true ;;
        *) die "no Bun binary for target: $TARGET" ;;
    esac

    local bun_bin="$RESOURCES/bun"
    $is_windows && bun_bin="$RESOURCES/bun.exe"
    if [[ -f "$bun_bin" ]]; then log "Bun already present — skipping."; return; fi

    command -v unzip &>/dev/null || die "'unzip' is required"
    local archive="${bun_target}.zip"
    local url="https://github.com/oven-sh/bun/releases/download/bun-v${BUN_VERSION}/${archive}"
    local tmp; tmp=$(mktemp -d); trap 'rm -rf "$tmp"' RETURN
    log "Downloading Bun ${BUN_VERSION} for ${TARGET} …"
    fetch "$url" "$tmp/$archive"
    unzip -q "$tmp/$archive" -d "$tmp"
    if $is_windows; then
        cp "$tmp/${bun_target}/bun.exe" "$bun_bin"
    else
        cp "$tmp/${bun_target}/bun" "$bun_bin"
        chmod +x "$bun_bin"
    fi
    log "Bun ready at $bun_bin"
}

log "Fetching dependencies for target: $TARGET"
fetch_postgres
fetch_bun
log "All dependencies ready in $RESOURCES"
