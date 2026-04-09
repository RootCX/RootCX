#!/bin/sh
# RootCX CLI installer — https://rootcx.com
# Usage: curl -fsSL https://rootcx.com/install.sh | sh
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
        echo "$1"
    else
        local latest
        latest=$(fetch "https://api.github.com/repos/${REPO}/releases/latest" \
            | grep '"tag_name"' | head -1 | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')
        [ -n "$latest" ] || error "could not determine latest version"
        echo "$latest"
    fi
}

# ─── Install ──────────────────────────────────────────────────────────────────

main() {
    need tar

    local target version url archive_name

    target=$(detect_target)
    version=$(resolve_version "$1")

    archive_name="rootcx-${target}.tar.gz"
    url="https://github.com/${REPO}/releases/download/${version}/${archive_name}"

    info "installing rootcx ${version} (${target})"

    mkdir -p "$BIN_DIR"

    local tmp
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT

    download "$url" "$tmp/$archive_name"
    tar -xzf "$tmp/$archive_name" -C "$BIN_DIR"
    chmod +x "$BIN_DIR/rootcx"

    green "rootcx ${version} installed to ${BIN_DIR}/rootcx"

    # ─── PATH setup ──────────────────────────────────────────────────────

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

    echo ""
    bold "to get started:"
    echo ""
    if ! command -v rootcx >/dev/null 2>&1; then
        info "  source ${rc_file:-~/.profile}"
    fi
    echo "  rootcx new my-app"
    echo "  cd my-app"
    echo "  rootcx connect https://your-core.rootcx.com"
    echo "  rootcx deploy"
    echo ""
}

main "$@"
