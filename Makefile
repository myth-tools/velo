# ═══════════════════════════════════════════════════════════════════════════
# Velo — Build, Development & Release Automation
# ═══════════════════════════════════════════════════════════════════════════
# Targets:
#   dev ............. Run Tauri dev mode (UI HMR + Rust hot-reload)
#   build ........... Build everything (UI + Rust workspace)
#   lint ............ Full quality pipeline (format, check, clippy, test)
#   test ............ Run Rust unit, integration, and doc tests
#   bundle-* ........ Produce platform installer packages
#   release ......... Full release workflow (lint → build → tag → publish)
#   clean/ prune .... Remove build artifacts
#   doctor .......... Verify all required tools are installed
#   help ............ Show this help
# ═══════════════════════════════════════════════════════════════════════════

SHELL          := /usr/bin/bash
SHELLFLAGS     := -euo pipefail -c
.ONESHELL:
.DELETE_ON_ERROR:
.DEFAULT_GOAL  := help
MAKEFLAGS      += --no-print-directory

# ─── Project Paths ──────────────────────────────────────────────────────
ROOT      := $(abspath $(dir $(lastword $(MAKEFILE_LIST))))
UI_DIR    := $(ROOT)/ui
BUILD_DIR := $(ROOT)/build
SCRIPTS   := $(ROOT)/scripts
TAURI_CONF := $(ROOT)/src-tauri/tauri.conf.json

# ─── Version & Identity ─────────────────────────────────────────────────
# Parsed from tauri.conf.json at evaluation time (:=), not recursively.
PYTHON    := $(shell command -v python3 2>/dev/null || command -v python 2>/dev/null || echo "")
ifneq ($(PYTHON),)
  VER := $(shell $(PYTHON) -c "import json; print(json.load(open('$(TAURI_CONF)'))['version'])" 2>/dev/null)
  BIN := $(shell $(PYTHON) -c "import json; print(json.load(open('$(TAURI_CONF)'))['productName'].lower())" 2>/dev/null)
endif
VER       := $(or $(VER),0.0.0)
BIN       := $(or $(BIN),velo)

# ─── OS Detection ───────────────────────────────────────────────────────
UNAME_S := $(shell uname -s)
BUILD_OS := unknown
ifneq ($(filter Linux,$(UNAME_S)),)
  BUILD_OS := linux
endif
ifneq ($(filter Darwin,$(UNAME_S)),)
  BUILD_OS := macos
endif
ifneq ($(filter MINGW% CYGWIN% MSYS%,$(UNAME_S)),)
  BUILD_OS := windows
endif

# ─── Tool Paths ─────────────────────────────────────────────────────────
CARGO    := cargo
PNPM     := pnpm
TAURI    := cargo tauri
CARGO_FLAGS := --locked

# Use sccache if available (RUSTC_WRAPPER full path avoids PATH lookup)
SCCACHE := $(shell command -v sccache 2>/dev/null)
ifneq ($(SCCACHE),)
  export RUSTC_WRAPPER := $(SCCACHE)
endif

# ─── Terminal Colors ────────────────────────────────────────────────────
# Detect if terminal supports color; fall back to plain text otherwise.
ifeq ($(filter $(MAKEFLAGS),s),)
  ifneq ($(TERM),)
    ifeq ($(shell tput colors 2>/dev/null || echo 0),0)
      bold :=  cyan :=  green :=  yellow :=  red :=  reset :=
    else
      bold  := $(shell tput bold 2>/dev/null)
      cyan  := $(shell tput setaf 6 2>/dev/null)
      green := $(shell tput setaf 2 2>/dev/null)
      yellow:= $(shell tput setaf 3 2>/dev/null)
      red   := $(shell tput setaf 1 2>/dev/null)
      reset := $(shell tput sgr0 2>/dev/null)
    endif
  endif
endif
# Assign empty fallback if still unset (e.g. non-TTY, -s flag)
bold   ?=
cyan   ?=
green  ?=
yellow ?=
red    ?=
reset  ?=


# ═══════════════════════════════════════════════════════════════════════════
# TARGETS
# ═══════════════════════════════════════════════════════════════════════════

# ─── Help ───────────────────────────────────────────────────────────────
.PHONY: help
help:
	@echo ""
	@printf "$(bold)$(cyan)━━━ Velo — Make Targets ━━━$(reset)\n"
	@echo ""
	@printf "$(bold)Development:$(reset)\n"
	@printf "  $(green)dev$(reset)           Run Tauri dev (UI HMR + Rust hot-reload)\n"
	@printf "  $(green)ui-dev$(reset)        Run UI dev server standalone (Vite)\n"
	@echo ""
	@printf "$(bold)Build:$(reset)\n"
	@printf "  $(green)build$(reset)         Build everything (UI + Rust)\n"
	@printf "  $(green)build-ui$(reset)      Build TypeScript frontend\n"
	@printf "  $(green)build-rust$(reset)    Build Rust workspace (debug profile)\n"
	@printf "  $(green)build-release$(reset)  Build Rust workspace (release profile)\n"
	@echo ""
	@printf "$(bold)Bundles [$(cyan)$(BUILD_OS)$(reset)]:$(reset)\n"
	@printf "  $(green)bundle-deb$(reset)    Build Linux .deb\n"
	@printf "  $(green)bundle-rpm$(reset)    Build Linux .rpm\n"
	@printf "  $(green)bundle-dmg$(reset)    Build macOS .dmg\n"
	@printf "  $(green)bundle-nsis$(reset)   Build Windows NSIS installer\n"
	@printf "  $(green)bundle-all$(reset)    Build all bundles for current OS\n"
	@echo ""
	@printf "$(bold)Quality:$(reset)\n"
	@printf "  $(green)lint$(reset)          Full quality pipeline (lint.sh)\n"
	@printf "  $(green)lint-quick$(reset)    Lint without build + test phases\n"
	@printf "  $(green)check$(reset)         cargo check + typecheck + clippy\n"
	@printf "  $(green)test$(reset)          Run Rust tests (unit + integration + doc)\n"
	@printf "  $(green)fmt$(reset)           Format Rust code (cargo fmt)\n"
	@printf "  $(green)clippy$(reset)        Run clippy (deny warnings)\n"
	@printf "  $(green)fix$(reset)           Auto-fix formatting (Rust + Biome)\n"
	@echo ""
	@printf "$(bold)Release:$(reset)\n"
	@printf "  $(green)release$(reset)       Full release (lint → build → tag → publish)\n"
	@printf "  $(green)release-dry-run$(reset) Preview release without side-effects\n"
	@echo ""
	@printf "$(bold)Diagnostics:$(reset)\n"
	@printf "  $(green)doctor$(reset)        Verify all required tools are installed\n"
	@printf "  $(green)info$(reset)          Show detected project configuration\n"
	@echo ""
	@printf "$(bold)Housekeeping:$(reset)\n"
	@printf "  $(green)clean$(reset)         Remove build artifacts\n"
	@printf "  $(green)clean-rust$(reset)    Remove Rust target directories\n"
	@printf "  $(green)clean-ui$(reset)      Remove UI node_modules + dist\n"
	@printf "  $(green)distclean$(reset)     Deep clean (target + cached deps)\n"
	@printf "  $(green)prune$(reset)         Aggressive cleanup (store prune, cargo clean)\n"
	@echo ""
	@printf "$(bold)Current:$(reset)  version=$(cyan)$(VER)$(reset)  bin=$(cyan)$(BIN)$(reset)  os=$(cyan)$(BUILD_OS)$(reset)\n"
	@echo ""

# ─── Development ────────────────────────────────────────────────────────
.PHONY: dev
dev: ui-install
	$(TAURI) dev

.PHONY: ui-dev
ui-dev:
	cd "$(UI_DIR)" && $(PNPM) dev

.PHONY: ui-install
ui-install:
	@printf "$(cyan)▶$(reset) installing frontend dependencies...\n"
	cd "$(UI_DIR)" && $(PNPM) install
	@printf "$(green)✓$(reset) frontend dependencies installed\n"

# ─── Build ──────────────────────────────────────────────────────────────
.PHONY: build
build: build-ui build-rust
	@printf "$(green)✓$(reset) build complete\n"

.PHONY: build-ui
build-ui: ui-install
	@printf "$(cyan)▶$(reset) building frontend...\n"
	cd "$(UI_DIR)" && $(PNPM) run build
	@printf "$(green)✓$(reset) frontend build complete\n"

.PHONY: build-rust
build-rust:
	@printf "$(cyan)▶$(reset) building Rust workspace (debug)...\n"
	$(CARGO) build $(CARGO_FLAGS) --workspace --all-targets
	@printf "$(green)✓$(reset) Rust build complete\n"

.PHONY: build-release
build-release:
	@printf "$(cyan)▶$(reset) building Rust workspace (release)...\n"
	$(CARGO) build $(CARGO_FLAGS) --release --workspace
	@printf "$(green)✓$(reset) release build complete\n"

# ─── Tauri Bundle Targets ───────────────────────────────────────────────
.PHONY: bundle-deb
bundle-deb: build-ui
	$(TAURI) build --bundles deb

.PHONY: bundle-rpm
bundle-rpm: build-ui
	$(TAURI) build --bundles rpm

.PHONY: bundle-dmg
bundle-dmg: build-ui
	$(TAURI) build --bundles dmg

.PHONY: bundle-nsis
bundle-nsis: build-ui
	$(TAURI) build --bundles nsis

.PHONY: bundle-all
bundle-all: build-ui
	@printf "$(cyan)▶$(reset) building all bundles for $(BUILD_OS)...\n"
	@bundles=""; \
	case "$(BUILD_OS)" in \
		linux)   bundles="deb,rpm" ;; \
		macos)   bundles="dmg" ;; \
		windows) bundles="nsis" ;; \
		*)       printf "$(red)✗$(reset) unsupported OS: $(BUILD_OS)\n"; exit 1 ;; \
	esac; \
	$(TAURI) build --bundles "$$bundles"

# ─── Quality ────────────────────────────────────────────────────────────
.PHONY: lint
lint:
	"$(SCRIPTS)/lint.sh"

.PHONY: lint-quick
lint-quick:
	"$(SCRIPTS)/lint.sh" --quick

.PHONY: check
check: check-rust check-ts clippy
	@printf "$(green)✓$(reset) all checks passed\n"

.PHONY: check-rust
check-rust:
	@printf "$(cyan)▶$(reset) cargo check...\n"
	$(CARGO) check $(CARGO_FLAGS) --workspace --all-targets

.PHONY: check-ts
check-ts:
	@printf "$(cyan)▶$(reset) TypeScript type check...\n"
	cd "$(UI_DIR)" && $(PNPM) typecheck

.PHONY: test
test:
	@printf "$(cyan)▶$(reset) running Rust tests...\n"
	$(CARGO) test $(CARGO_FLAGS) --workspace --lib --bins --tests --examples
	$(CARGO) test $(CARGO_FLAGS) --doc
	@printf "$(green)✓$(reset) all tests passed\n"

.PHONY: fmt
fmt:
	$(CARGO) fmt --all

.PHONY: clippy
clippy:
	@printf "$(cyan)▶$(reset) running clippy...\n"
	$(CARGO) clippy $(CARGO_FLAGS) --workspace --all-targets -- -D warnings

.PHONY: fix
fix:
	@printf "$(cyan)▶$(reset) auto-fixing formatting...\n"
	$(CARGO) fmt --all
	cd "$(UI_DIR)" && $(PNPM) biome check --write src/ vite.config.ts 2>/dev/null || true
	@printf "$(green)✓$(reset) fix complete\n"

# ─── Release ────────────────────────────────────────────────────────────
.NOTPARALLEL: release release-dry-run

.PHONY: release
release:
	"$(SCRIPTS)/release.sh"

.PHONY: release-dry-run
release-dry-run:
	"$(SCRIPTS)/release.sh" --dry-run --skip-editor

# ─── Diagnostics ────────────────────────────────────────────────────────
.PHONY: doctor
doctor:
	@printf "$(bold)$(cyan)━━━ Velo Doctor — Tool Verification ━━━$(reset)\n"
	@failed=0; \
	for tool in cargo rustc pnpm node python3; do \
		if command -v "$$tool" >/dev/null 2>&1; then \
			printf "  $(green)✓$(reset) %-12s found\n" "$$tool"; \
		else \
			printf "  $(red)✗$(reset) %-12s NOT found\n" "$$tool"; \
			failed=$$((failed + 1)); \
		fi; \
	done; \
	if command -v sccache >/dev/null 2>&1; then \
		printf "  $(green)✓$(reset) %-12s found (%s)\n" "sccache" "$$(command -v sccache)"; \
	else \
		printf "  $(yellow)⚠$(reset) %-12s not installed (recommended)\n" "sccache"; \
	fi; \
	printf "$(cyan)▶$(reset) %s\n" "$$(rustc --version 2>/dev/null || echo 'rustc N/A')"; \
	printf "$(cyan)▶$(reset) %s\n" "$$(cargo --version 2>/dev/null || echo 'cargo N/A')"; \
	printf "$(cyan)▶$(reset) %s\n" "$$(node --version 2>/dev/null || echo 'node N/A')"; \
	printf "$(cyan)▶$(reset) %s\n" "$$(pnpm --version 2>/dev/null || echo 'pnpm N/A')"; \
	if [ "$$failed" -gt 0 ]; then \
		printf "$(red)✗$(reset) %d tool(s) missing — install them first\n" "$$failed"; \
		exit 1; \
	fi; \
	printf "$(green)✓$(reset) all required tools present\n"

.PHONY: info
info:
	@printf "$(bold)$(cyan)━━━ Velo Configuration ━━━$(reset)\n"
	@printf "  $(bold)Root:$(reset)      %s\n" "$(ROOT)"
	@printf "  $(bold)Version:$(reset)   %s\n" "$(VER)"
	@printf "  $(bold)Binary:$(reset)    %s\n" "$(BIN)"
	@printf "  $(bold)OS:$(reset)        %s\n" "$(BUILD_OS)"
	@printf "  $(bold)Python:$(reset)    %s\n" "$(or $(PYTHON),not found)"
	@printf "  $(bold)UI Dir:$(reset)    %s\n" "$(UI_DIR)"
	@printf "  $(bold)Build Dir:$(reset) %s\n" "$(BUILD_DIR)"
	@printf "  $(bold)Config:$(reset)    %s\n" "$(TAURI_CONF)"
	@printf "  $(bold)Cargo Flags:$(reset) %s\n" "$(CARGO_FLAGS)"
	@if command -v sccache >/dev/null 2>&1; then \
		printf "  $(bold)Sccache:$(reset)   $(green)active$(reset) (%s)\n" "$$(command -v sccache)"; \
	else \
		printf "  $(bold)Sccache:$(reset)   $(yellow)not installed$(reset)\n"; \
	fi

# ─── Housekeeping ───────────────────────────────────────────────────────
.PHONY: clean
clean: clean-rust clean-ui
	@printf "$(cyan)▶$(reset) removing build artifacts...\n"
	rm -rf "$(BUILD_DIR)"
	@printf "$(green)✓$(reset) clean complete\n"

.PHONY: clean-rust
clean-rust:
	@printf "$(cyan)▶$(reset) cleaning Rust target directories...\n"
	rm -rf "$(ROOT)/target"
	rm -rf "$(ROOT)/src-tauri/target"
	@printf "$(green)✓$(reset) Rust artifacts removed\n"

.PHONY: clean-ui
clean-ui:
	@printf "$(cyan)▶$(reset) cleaning UI artifacts...\n"
	rm -rf "$(UI_DIR)/node_modules"
	rm -rf "$(UI_DIR)/dist"
	rm -rf "$(UI_DIR)/.vite"
	@printf "$(green)✓$(reset) UI artifacts removed\n"

.PHONY: distclean
distclean: clean
	@printf "$(cyan)▶$(reset) deep clean — removing cached dependencies...\n"
	$(CARGO) cache --autoclean 2>/dev/null || true
	@printf "$(green)✓$(reset) distclean complete\n"

.PHONY: prune
prune:
	@printf "$(cyan)▶$(reset) aggressive cleanup...\n"
	$(CARGO) clean 2>/dev/null || true
	rm -rf "$(ROOT)/target" "$(ROOT)/src-tauri/target"
	rm -rf "$(UI_DIR)/node_modules" "$(UI_DIR)/dist" "$(UI_DIR)/.vite"
	cd "$(UI_DIR)" && $(PNPM) store prune 2>/dev/null || true
	@printf "$(green)✓$(reset) prune complete\n"

# ─── Safety ─────────────────────────────────────────────────────────────
# Prevent implicit rules from treating these as targets
.PHONY: Makefile Makefile.*
