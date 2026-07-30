#!/bin/sh
# RootCX CLI installer — https://rootcx.com
# Usage: curl -fsSL https://rootcx.com/install.sh | sh
#        curl -fsSL https://rootcx.com/install.sh | sh -s -- --no-path-update
#
# Installs the rootcx binary to ~/.rootcx/bin and adds it to PATH.
# Requires: curl (or wget), tar, and a POSIX shell.

set -e

# ─── Config ───────────────────────────────────────────────────────────────────

REPO="RootCX/RootCX"
INSTALL_DIR="${ROOTCX_INSTALL:-$HOME/.rootcx}"
BIN_DIR="$INSTALL_DIR/bin"

# ─── Helpers ──────────────────────────────────────────────────────────────────

info()  { printf '\033[0;2m%s\033[0m\n' "$*"; }
bold()  { printf '\033[1m%s\033[0m\n' "$*"; }
green() { printf '\033[1;32m%s\033[0m\n' "$*"; }
error() { printf '\033[0;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

need() { command -v "$1" >/dev/null 2>&1 || error "$1 is required but not found"; }

fetch() {
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$1"
    elif command -v wget >/dev/null 2>&1; then
        wget -qO- "$1"
    else
        error "curl or wget is required"
    fi
}

download() {
    local url="$1" out="$2"
    if command -v curl >/dev/null 2>&1; then
        curl --fail --location --progress-bar --output "$out" "$url"
    else
        wget --show-progress -qO "$out" "$url"
    fi
}

try_download() {
    if command -v curl >/dev/null 2>&1; then
        curl --fail --location --silent --output "$2" "$1"
    else
        wget -qO "$2" "$1"
    fi
}

release_asset_checksum() {
    fetch "https://api.github.com/repos/${REPO}/releases/tags/$1" \
        | awk -v archive="$2" '
            $0 ~ "\"name\"[[:space:]]*:[[:space:]]*\"" archive "\"" { found = 1 }
            found && /"digest"[[:space:]]*:[[:space:]]*"sha256:[0-9a-fA-F]+"/ {
                value = $0
                sub(/^.*"digest"[[:space:]]*:[[:space:]]*"sha256:/, "", value)
                sub(/".*$/, "", value)
                print tolower(value)
                exit
            }
        '
}

verify_checksum() {
    archive_path="$1"
    expected="$2"
    [ -n "$expected" ] || error "checksum for $(basename "$archive_path") is missing"

    if command -v sha256sum >/dev/null 2>&1; then
        actual=$(sha256sum "$archive_path" | awk '{ print $1 }')
    elif command -v shasum >/dev/null 2>&1; then
        actual=$(shasum -a 256 "$archive_path" | awk '{ print $1 }')
    else
        error "sha256sum or shasum is required"
    fi

    [ "$actual" = "$expected" ] || error "checksum verification failed for $(basename "$archive_path")"
}

# ─── Detect platform ─────────────────────────────────────────────────────────

detect_target() {
    case "$(uname -s)" in
        Darwin) os="apple-darwin" ;;
        Linux)  os="unknown-linux-gnu" ;;
        *)      error "unsupported OS: $(uname -s)" ;;
    esac

    case "$(uname -m)" in
        x86_64|amd64)  arch="x86_64" ;;
        arm64|aarch64) arch="aarch64" ;;
        *)             error "unsupported architecture: $(uname -m)" ;;
    esac

    echo "${arch}-${os}"
}

# ─── Resolve version ─────────────────────────────────────────────────────────

resolve_version() {
    if [ -n "$1" ]; then
        echo "cli-v$(echo "$1" | sed 's/^cli-v//; s/^v//')"
    else
        local latest
        latest=$(fetch "https://api.github.com/repos/${REPO}/releases?per_page=100" \
            | grep '"tag_name"' | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/' \
            | grep '^cli-v' | head -1)
        [ -n "$latest" ] || error "could not determine latest CLI version"
        echo "$latest"
    fi
}

# ─── Install ──────────────────────────────────────────────────────────────────

main() {
    need tar

    version_arg=""
    update_path=true
    while [ "$#" -gt 0 ]; do
        case "$1" in
            --no-path-update) update_path=false ;;
            -*) error "unknown option: $1" ;;
            *)
                [ -z "$version_arg" ] || error "only one version may be specified"
                version_arg="$1"
                ;;
        esac
        shift
    done

    target=$(detect_target)
    version=$(resolve_version "$version_arg")

    archive_name="rootcx-${target}.tar.gz"
    release_url="https://github.com/${REPO}/releases/download/${version}"

    info "installing rootcx ${version} (${target})"

    mkdir -p "$INSTALL_DIR" "$BIN_DIR"

    tmp=$(mktemp -d "$INSTALL_DIR/install.XXXXXX")
    trap 'rm -rf "$tmp"' EXIT

    download "$release_url/$archive_name" "$tmp/$archive_name"
    if try_download "$release_url/SHA256SUMS" "$tmp/SHA256SUMS"; then
        expected=$(awk -v archive="$archive_name" '$2 == archive || $2 == "*" archive { print $1; exit }' "$tmp/SHA256SUMS")
    else
        expected=$(release_asset_checksum "$version" "$archive_name")
    fi
    verify_checksum "$tmp/$archive_name" "$expected"

    mkdir "$tmp/extracted"
    tar -xzf "$tmp/$archive_name" -C "$tmp/extracted"
    [ -f "$tmp/extracted/rootcx" ] || error "rootcx binary is missing from the release archive"
    chmod +x "$tmp/extracted/rootcx"
    mv "$tmp/extracted/rootcx" "$BIN_DIR/rootcx"

    green "rootcx ${version} installed to ${BIN_DIR}/rootcx"

    # ─── PATH setup ──────────────────────────────────────────────────────

    if [ "$update_path" = true ]; then
        case ":$PATH:" in
            *":$BIN_DIR:"*) ;; # already in PATH
            *)
                local shell_name rc_file
                shell_name=$(basename "${SHELL:-/bin/sh}")

                case "$shell_name" in
                    zsh)  rc_file="$HOME/.zshrc" ;;
                    bash) rc_file="$HOME/.bashrc"
                          [ -f "$HOME/.bash_profile" ] && rc_file="$HOME/.bash_profile" ;;
                    fish) rc_file="$HOME/.config/fish/config.fish" ;;
                    *)    rc_file="" ;;
                esac

                if [ -n "$rc_file" ] && ! grep -q "# rootcx" "$rc_file" 2>/dev/null; then
                    case "$shell_name" in
                        fish)
                            echo "" >> "$rc_file"
                            echo "# rootcx" >> "$rc_file"
                            echo "set -gx PATH $BIN_DIR \$PATH" >> "$rc_file"
                            ;;
                        *)
                            echo "" >> "$rc_file"
                            echo "# rootcx" >> "$rc_file"
                            echo "export PATH=\"$BIN_DIR:\$PATH\"" >> "$rc_file"
                            ;;
                    esac
                    info "added $BIN_DIR to PATH in $rc_file"
                else
                    echo ""
                    bold "add this to your shell profile:"
                    echo "  export PATH=\"$BIN_DIR:\$PATH\""
                fi
                ;;
        esac
    fi

    echo ""
    printf '\033[1;32m✓\033[0m rootcx installed successfully\n'
    echo ""
}

main "$@"
