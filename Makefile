# RootCX Core and CLI development. The UI is built in the separate rootcx-ui repo.
.DEFAULT_GOAL := release

TARGET_MAC_ARM := aarch64-apple-darwin
TARGET_MAC_X86 := x86_64-apple-darwin
TARGET_LINUX := x86_64-unknown-linux-gnu
TARGET_LINUX_ARM := aarch64-unknown-linux-gnu
TARGET_WIN := x86_64-pc-windows-msvc

ifeq ($(OS),Windows_NT)
  _HOSTLINE := $(shell rustc -vV 2>nul | findstr /B "host:")
  HOST := $(lastword $(_HOSTLINE))
else
  HOST := $(shell rustc -vV 2>/dev/null | awk '/^host:/{print $$2}')
endif

.PHONY: release dev dev-core dev-mode prod-mode deps \
        deps-mac-arm deps-mac-x86 deps-linux deps-linux-arm deps-win \
        test test-integration test-image core-check core-check-tests core-test \
        core-unit core-governance core-verify core-mutations

release:
	cargo build --locked --release -p rootcx-core -p rootcx-cli

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
	CARGO_JOBS="$(CARGO_JOBS)" TEST_THREADS="$(TEST_THREADS)" bash scripts/core-test.sh integration "$(TEST)" "$(FILTER)"

core-unit:
	CARGO_JOBS="$(CARGO_JOBS)" TEST_THREADS="$(TEST_THREADS)" bash scripts/core-test.sh unit "" "$(FILTER)"

core-governance:
	CARGO_JOBS="$(CARGO_JOBS)" TEST_THREADS="$(TEST_THREADS)" bash scripts/core-test.sh integration governance_test "$(FILTER)"

test core-verify:
	CARGO_JOBS="$(CARGO_JOBS)" TEST_THREADS="$(TEST_THREADS)" bash scripts/core-test.sh verify

core-mutations:
	CARGO_JOBS="$(CARGO_JOBS)" bash scripts/core-test.sh mutations $(FILTER)

test-integration: core-governance

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

dev: dev-core

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
