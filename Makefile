# RootCX build system — works natively on macOS, Linux, and Windows (cmd.exe).
#
#   make release         # auto-detect platform, build native package
#   make dev             # debug loop on current host
#   make dev-mode        # CLI + Claude Code plugin → local dev builds
#   make prod-mode       # CLI + Claude Code plugin → production
#   make deps            # download PG + Bun for current host
#   make core-check      # fast Core library check
#   make core-verify     # staged Core checks, tests, and timing output
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

.PHONY: test-image core-check core-check-tests core-test core-unit core-governance core-verify \
	release dev deps dev-mode prod-mode \
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

# ── Core verification ─────────────────────────────────────────────────────────
#
# Leave CARGO_JOBS empty to let Cargo select the host's normal parallelism.
# Set it explicitly when running on a constrained machine, for example:
#   make core-check-tests CARGO_JOBS=2
# Cargo's timing report is written under target/cargo-timings/.

CARGO_JOB_ARGS := $(if $(CARGO_JOBS),--jobs $(CARGO_JOBS),)
TEST_THREADS ?= 2

core-check:
	@echo "[core/check] checking the Core library"
	cargo check -p rootcx-core --lib $(CARGO_JOB_ARGS)

core-check-tests:
	@echo "[core/check-tests] compiling Core test targets (timings enabled)"
	cargo check -p rootcx-core --tests $(CARGO_JOB_ARGS) --timings
	@echo "[core/check-tests] timing report: target/cargo-timings/"

core-test:
	@test -n "$(TEST)" || { echo "usage: make core-test TEST=governance_test FILTER=cross_app_grants_test" >&2; exit 2; }
	@echo "[core/test] running $(TEST)"
	cargo test -p rootcx-core --test $(TEST) $(CARGO_JOB_ARGS) -- "$(FILTER)" --test-threads=$(TEST_THREADS) --nocapture

core-unit:
	@echo "[core/unit] running Core library tests against disposable PostgreSQL"
	bash scripts/core-unit.sh $(CARGO_JOB_ARGS)

core-governance:
	@echo "[core/governance] real PostgreSQL, HTTP and worker boundaries"
	cargo test -p rootcx-core $(CARGO_JOB_ARGS) --test governance_test \
		-- "$(FILTER)" --test-threads=$(TEST_THREADS) $(if $(TEST_TIMINGS),--nocapture,)

core-verify:
	@echo "[core/verify] phase 1/4 — compile and run Core library tests"
	$(MAKE) core-unit CARGO_JOBS="$(CARGO_JOBS)"
	@echo "[core/verify] phase 2/4 — compile and run governance regressions"
	$(MAKE) core-governance CARGO_JOBS="$(CARGO_JOBS)" FILTER= TEST_THREADS="$(TEST_THREADS)"
	@echo "[core/verify] phase 3/4 — run worker prelude tests"
	bun test core/src/backend_prelude.test.ts
	@echo "[core/verify] phase 4/4 — check patch whitespace"
	git diff --check
	@echo "[core/verify] all local verification phases passed"

# ── Development ───────────────────────────────────────────────────────────────

DEV_DB := postgres://rootcx:rootcx@localhost:5480/rootcx
DEV_COMPOSE := docker compose -f docker-compose.dev.yml
DEV_POSTGRES_READY := $(DEV_COMPOSE) exec -T postgres pg_isready -h 127.0.0.1 -U rootcx -d rootcx

CLAUDE_SETTINGS := $(HOME)/.claude/settings.json
define set-plugin # $(1) = plugin key
	@command -v jq >/dev/null || { echo "jq required: brew install jq"; exit 1; }
	@jq '.enabledPlugins = {"$(1)": true}' $(CLAUDE_SETTINGS) > $(CLAUDE_SETTINGS).tmp \
		&& mv $(CLAUDE_SETTINGS).tmp $(CLAUDE_SETTINGS)
endef

dev-mode:
ifndef PLUGIN_DIR
	$(error PLUGIN_DIR is required — path to your local claude-code-plugin checkout. Usage: make dev-mode PLUGIN_DIR=/path/to/claude-code-plugin)
endif
	cargo build -p rootcx-cli
	@mkdir -p $(HOME)/.local/bin
	ln -sf $(CURDIR)/target/debug/rootcx $(HOME)/.local/bin/rootcx
	@jq '.extraKnownMarketplaces["rootcx-local"].source = {"source":"directory","path":"$(PLUGIN_DIR)"}' \
		$(CLAUDE_SETTINGS) > $(CLAUDE_SETTINGS).tmp && mv $(CLAUDE_SETTINGS).tmp $(CLAUDE_SETTINGS)
	$(call set-plugin,rootcx@rootcx-local)
	@rm -rf $(HOME)/.claude/plugins/cache/rootcx-local
	@echo "✓ dev mode: CLI → target/debug, plugin → $(PLUGIN_DIR) (cache cleared)"

prod-mode:
	@if [ -L $(HOME)/.local/bin/rootcx ]; then \
		rm $(HOME)/.local/bin/rootcx; echo "✓ removed dev CLI symlink"; \
	fi
	$(call set-plugin,rootcx@rootcx)
	@echo "✓ prod mode: plugin → GitHub rootcx"

dev:
	pnpm --dir runtime/ui install
	pnpm --dir studio/ui install
	cargo tauri dev

dev-core:
	$(DEV_COMPOSE) up -d
	@echo "Waiting for Postgres..."; \
	attempts=0; \
	until $(DEV_POSTGRES_READY) >/dev/null 2>&1; do \
		attempts=$$((attempts + 1)); \
		if ! $(DEV_COMPOSE) ps --status running --services | grep -qx postgres; then \
			echo "Postgres stopped before becoming ready:"; \
			$(DEV_COMPOSE) logs --no-color --tail=100 postgres; \
			exit 1; \
		fi; \
		if [ "$$attempts" -ge 60 ]; then \
			echo "Postgres did not become ready within 30 seconds:"; \
			$(DEV_COMPOSE) logs --no-color --tail=100 postgres; \
			exit 1; \
		fi; \
		sleep 0.5; \
	done
	DATABASE_URL=$(DEV_DB) \
	ROOTCX_TENANT_REF=$${ROOTCX_TENANT_REF:-local} \
	ROOTCX_OIDC_ISSUER=$${ROOTCX_OIDC_ISSUER:-http://localhost:3000} \
	ROOTCX_OIDC_CLIENT_ID=$${ROOTCX_OIDC_CLIENT_ID:-rootcx-local} \
	ROOTCX_OIDC_CLIENT_SECRET=$${ROOTCX_OIDC_CLIENT_SECRET:-xMh0Aoj2Qa6eB5quFm-Y_K4b62bcHUV5wypqGMCCiUc} \
	ROOTCX_ASSISTANT_DIR=$${ROOTCX_ASSISTANT_DIR:-$(CURDIR)/../ai_agent_base/backend} \
	ROOTCX_LLM_ENDPOINT=$${ROOTCX_LLM_ENDPOINT:-http://localhost:3000/api/llm} \
	cargo run -p rootcx-core

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
