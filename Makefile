# RootCX build system — works natively on macOS, Linux, and Windows (cmd.exe).
#
#   make release         # auto-detect platform, build native package
#   make dev             # debug loop on current host
#   make deps            # download PG + Bun for current host
#   make dist-mac-arm    # .dmg  (Apple Silicon)    — requires macOS
#   make dist-mac-x86    # .dmg  (Intel Mac)        — requires macOS
#   make dist-mac-uni    # .dmg  (universal)         — requires macOS
#   make dist-linux      # .AppImage + .deb (x86-64) — requires Linux
#   make dist-linux-arm  # .AppImage + .deb (arm64)  — requires Linux
#   make dist-win        # .exe  NSIS installer      — requires Windows
#
# On Windows: run from "Developer PowerShell for VS 2022" for MSVC linker.

TARGET_MAC_ARM   := aarch64-apple-darwin
TARGET_MAC_X86   := x86_64-apple-darwin
TARGET_LINUX     := x86_64-unknown-linux-gnu
TARGET_LINUX_ARM := aarch64-unknown-linux-gnu
TARGET_WIN       := x86_64-pc-windows-msvc

# ── Cross-platform host detection (no bash dependency) ───────────────────────

ifeq ($(OS),Windows_NT)
  _HOSTLINE := $(shell rustc -vV 2>nul | findstr /B "host:")
  HOST := $(lastword $(_HOSTLINE))
else
  HOST := $(shell rustc -vV 2>/dev/null | awk '/^host:/{print $$2}')
endif

ifeq ($(HOST),)
  $(error rustc not found in PATH — install from https://rustup.rs)
endif

DIST := target/dist

.PHONY: test-image release dev deps \
        deps-mac-arm deps-mac-x86 deps-linux deps-linux-arm deps-win \
        require-mac require-linux require-win \
        build-frontend \
        dist-mac-arm dist-mac-x86 dist-mac-uni \
        dist-linux dist-linux-arm dist-win

# ── Auto-detect platform ─────────────────────────────────────────────────────

release:
ifneq ($(findstring aarch64-apple-darwin,$(HOST)),)
	@$(MAKE) dist-mac-arm
else ifneq ($(findstring x86_64-apple-darwin,$(HOST)),)
	@$(MAKE) dist-mac-x86
else ifneq ($(findstring linux-gnu,$(HOST)),)
	@$(MAKE) dist-linux
else ifneq ($(findstring windows,$(HOST)),)
	@$(MAKE) dist-win
else
	$(error Unsupported host: $(HOST))
endif

# ── Test infrastructure ───────────────────────────────────────────────────────

test-image:
	docker compose build postgres

# ── Development ───────────────────────────────────────────────────────────────

dev:
	pnpm --dir runtime/ui install
	pnpm --dir studio/ui install
	cargo tauri dev

# ── Resource dependencies (PostgreSQL + Bun) ──────────────────────────────────

ifeq ($(OS),Windows_NT)
  FETCH = powershell -ExecutionPolicy Bypass -File scripts/fetch-deps.ps1
else
  FETCH = scripts/fetch-deps.sh
endif

deps:          ; $(FETCH) $(HOST)
deps-mac-arm:  ; $(FETCH) $(TARGET_MAC_ARM)
deps-mac-x86:  ; $(FETCH) $(TARGET_MAC_X86)
deps-linux:    ; $(FETCH) $(TARGET_LINUX)
deps-linux-arm: ; $(FETCH) $(TARGET_LINUX_ARM)
deps-win:      ; $(FETCH) $(TARGET_WIN)

# ── Host guards (pure Make functions, no shell syntax) ────────────────────────

require-mac:
	$(if $(findstring apple-darwin,$(HOST)),,$(error dist-mac requires macOS — current host: $(HOST)))

require-linux:
	$(if $(findstring linux,$(HOST)),,$(error dist-linux requires Linux — current host: $(HOST)))

require-win:
	$(if $(findstring windows,$(HOST)),,$(error dist-win requires Windows — current host: $(HOST)))

# ── Distribution ──────────────────────────────────────────────────────────────

ifeq ($(OS),Windows_NT)
$(DIST):
	-mkdir target\dist
else
$(DIST):
	mkdir -p $@
endif

# Build frontend explicitly (Tauri's beforeBuildCommand resolves CWD incorrectly
# in workspace setups). We build ourselves, then skip it via --config override.
TAURI_BUILD_CFG := --config studio/src-tauri/tauri.build.json

build-frontend:
	pnpm --dir studio/ui install
	pnpm --dir studio/ui build

dist-mac-arm: require-mac build-frontend
	cargo tauri build --target $(TARGET_MAC_ARM) --bundles dmg $(TAURI_BUILD_CFG)
	@echo "" && ls target/$(TARGET_MAC_ARM)/release/bundle/dmg/*.dmg

dist-mac-x86: require-mac build-frontend
	cargo tauri build --target $(TARGET_MAC_X86) --bundles dmg $(TAURI_BUILD_CFG)
	@echo "" && ls target/$(TARGET_MAC_X86)/release/bundle/dmg/*.dmg

dist-mac-uni: require-mac build-frontend
	cargo tauri build --target universal-apple-darwin --bundles dmg $(TAURI_BUILD_CFG)
	@echo "" && ls target/universal-apple-darwin/release/bundle/dmg/*.dmg

dist-linux: require-linux build-frontend $(DIST)
	cargo tauri build --target $(TARGET_LINUX) --bundles appimage,deb $(TAURI_BUILD_CFG)
	@img=$$(ls target/$(TARGET_LINUX)/release/bundle/appimage/*.AppImage 2>/dev/null | head -1) && \
	 [ -n "$$img" ] && tar -czf $(DIST)/rootcx-studio-linux-x86_64.tar.gz \
	   -C "$$(dirname $$img)" "$$(basename $$img)"
	@echo "" && ls $(DIST)/rootcx-studio-linux-x86_64.tar.gz \
	  target/$(TARGET_LINUX)/release/bundle/deb/*.deb 2>/dev/null

dist-linux-arm: require-linux build-frontend $(DIST)
	cargo tauri build --target $(TARGET_LINUX_ARM) --bundles appimage,deb $(TAURI_BUILD_CFG)
	@img=$$(ls target/$(TARGET_LINUX_ARM)/release/bundle/appimage/*.AppImage 2>/dev/null | head -1) && \
	 [ -n "$$img" ] && tar -czf $(DIST)/rootcx-studio-linux-aarch64.tar.gz \
	   -C "$$(dirname $$img)" "$$(basename $$img)"
	@echo "" && ls $(DIST)/rootcx-studio-linux-aarch64.tar.gz \
	  target/$(TARGET_LINUX_ARM)/release/bundle/deb/*.deb 2>/dev/null

dist-win: require-win build-frontend
	cargo tauri build --target $(TARGET_WIN) --bundles nsis $(TAURI_BUILD_CFG)
ifeq ($(OS),Windows_NT)
	@dir target\$(TARGET_WIN)\release\bundle\nsis\*.exe
else
	@echo "" && ls target/$(TARGET_WIN)/release/bundle/nsis/*.exe
endif
