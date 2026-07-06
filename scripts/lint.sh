#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────────────────
# 🚀 Velo — Industry-Grade Quality Verification & Lint [v1.0]
# ──────────────────────────────────────────────────────────────────────────────
#
# Performs a full workspace audit: formatting, static analysis, linting,
# building, testing, and documentation generation.
#
# Features:
#   - ANSI Colorized Status Reporting (respects NO_COLOR / non-TTY)
#   - Execution Timing for Performance Auditing
#   - Summary Dashboard with Health Statistics
#   - Idempotent — safe to run from any working directory
#
# Usage:
#   ./scripts/lint.sh              # Run full lint & verification pipeline
#   ./scripts/lint.sh --quick      # Skip build + test phases for rapid iteration
#   ./scripts/lint.sh --fix        # Apply auto-fixes (cargo fmt, trailing ws)
#
# ──────────────────────────────────────────────────────────────────────────────

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "$WORKSPACE_ROOT"

# ── Environment Configuration ─────────────────────────────────────────────────
set -euo pipefail
export RUST_BACKTRACE=1

# Use sccache if available for faster incremental builds
if command -v sccache &>/dev/null; then
	export RUSTC_WRAPPER=sccache
fi

# Flags
QUICK_MODE=false
FIX_MODE=false
for arg in "$@"; do
	case "$arg" in
	--quick) QUICK_MODE=true ;;
	--fix) FIX_MODE=true ;;
	esac
done

# ── ANSI Color Support ────────────────────────────────────────────────────────
if [[ -t 1 && -z "${NO_COLOR:-}" ]]; then
	export FORCE_COLOR=1
	BOLD="\033[1m"
	GREEN="\033[32m"
	BLUE="\033[34m"
	YELLOW="\033[33m"
	RED="\033[31m"
	MAGENTA="\033[35m"
	CYAN="\033[36m"
	NC="\033[0m"
else
	unset FORCE_COLOR
	BOLD=""
	GREEN=""
	BLUE=""
	YELLOW=""
	RED=""
	MAGENTA=""
	CYAN=""
	NC=""
fi

# ── Timer ─────────────────────────────────────────────────────────────────────
STAMP_START=$(date +%s)

# ── OS Detection ──────────────────────────────────────────────────────────────
OS_UPPER=$(uname -s | tr '[:lower:]' '[:upper:]')
case "$OS_UPPER" in
LINUX*) PLATFORM="Linux" ;;
DARWIN*) PLATFORM="macOS" ;;
*) PLATFORM="Unknown ($OS_UPPER)" ;;
esac

# ── Helper Functions ──────────────────────────────────────────────────────────

header() {
	echo -e "\n${BOLD}${CYAN}────────────────────────────────────────────────────────────────────────────────${NC}"
	echo -e "${BOLD}${MAGENTA}  $1 ${NC}"
	echo -e "${BOLD}${CYAN}────────────────────────────────────────────────────────────────────────────────${NC}"
}

status() {
	local label=$1
	local color=$2
	local icon=$3
	echo -e "[ ${color}${icon}${NC} ] ${BOLD}${label}...${NC}"
}

report_ok() {
	echo -e " [ ${GREEN}PASS${NC} ]"
}

warn() {
	echo -e " [ ${YELLOW}WARN${NC} ] $1"
}

report_fail() {
	echo -e "\n${RED}${BOLD}❌ VERIFICATION FAILED at: $1${NC}\n"
	exit 1
}

check_cmd() {
	command -v "$1" &>/dev/null
}

run_step() {
	local label="$1"
	shift
	if "$@"; then
		report_ok
	else
		echo -e "${RED}FAILED: $label${NC}" >&2
		report_fail "$label"
	fi
}

run_step_optional() {
	local label="$1"
	local tool="$2"
	shift 2
	if check_cmd "$tool"; then
		run_step "$label" "$@"
	else
		echo -e " [ ${YELLOW}SKIP${NC} ] ${tool} not installed"
	fi
}

# ── Entrance Banner ───────────────────────────────────────────────────────────

echo -e "${BOLD}${CYAN}⚡ Velo Quality Auditor [Platform: ${YELLOW}${PLATFORM}${BOLD}${CYAN}] [Root: ${YELLOW}${WORKSPACE_ROOT}${BOLD}${CYAN}]${NC}"
if $QUICK_MODE; then
	echo -e "${YELLOW}  Quick mode enabled — skipping build + test phases${NC}"
fi
if $FIX_MODE; then
	echo -e "${YELLOW}  Fix mode enabled — auto-formatting will be applied${NC}"
fi

# ══════════════════════════════════════════════════════════════════════════════
# Phase 0: Code Formatting
# ══════════════════════════════════════════════════════════════════════════════
header "Phase 0: Code Formatting"

status "Stripping trailing whitespace from Rust sources" "${YELLOW}" "🧹"
if [[ "$PLATFORM" == "macOS" ]]; then
	find . -path ./target -prune -o -path ./node_modules -prune -o -name "*.rs" -exec sed -i '' 's/[[:space:]]*$//' {} +
else
	find . -path ./target -prune -o -path ./node_modules -prune -o -name "*.rs" -exec sed -i 's/[[:space:]]*$//' {} +
fi
report_ok

status "Formatting Rust sources" "${CYAN}" "🎨"
run_step "cargo fmt" cargo fmt --all
FMT_STATUS="${GREEN}CLEAN${NC}"

status "Formatting TypeScript sources" "${CYAN}" "🎨"
if [ -f "ui/node_modules/.bin/biome" ] && check_cmd pnpm; then
	run_step "biome check --write" bash -c 'cd ui && pnpm biome check --write src/ vite.config.ts'
else
	echo -e " [ ${YELLOW}SKIP${NC} ] biome not installed in ui/"
fi

status "Linting shell scripts" "${YELLOW}" "🐚"
run_step_optional "shellcheck" shellcheck \
	find . -path ./target -prune -o -path ./node_modules -prune -o -type f \
	\( -name "*.sh" \) -exec shellcheck {} +

# ══════════════════════════════════════════════════════════════════════════════
# Phase 1: Static Analysis & Type Checking
# ══════════════════════════════════════════════════════════════════════════════
header "Phase 1: Static Analysis & Type Checking"

status "Rust cargo check (workspace)" "${BLUE}" "🔍"
run_step "cargo check" cargo check --workspace --all-targets
CARGO_CHECK_STATUS="${GREEN}CLEAN${NC}"

status "TypeScript type checking" "${BLUE}" "🔍"
if [ -d "ui" ] && check_cmd pnpm; then
	run_step "tsc --noEmit" bash -c 'cd ui && pnpm typecheck'
	TS_STATUS="${GREEN}CLEAN${NC}"
else
	echo -e " [ ${YELLOW}SKIP${NC} ] ui/ or pnpm not available"
	TS_STATUS="${YELLOW}NOT CHECKED${NC}"
fi

status "Biome lint" "${BLUE}" "🔍"
if [ -f "ui/node_modules/.bin/biome" ] && check_cmd pnpm; then
	run_step "biome check" bash -c 'cd ui && pnpm biome check src/ vite.config.ts'
	BIOME_STATUS="${GREEN}CLEAN${NC}"
else
	echo -e " [ ${YELLOW}SKIP${NC} ] biome not installed in ui/"
	BIOME_STATUS="${YELLOW}NOT CHECKED${NC}"
fi

# ══════════════════════════════════════════════════════════════════════════════
# Phase 2: Linting & Best Practices
# ══════════════════════════════════════════════════════════════════════════════
header "Phase 2: Linting & Best Practices"

status "Rust Clippy (deny warnings)" "${YELLOW}" "✨"
run_step "cargo clippy" cargo clippy --workspace --all-targets --locked -- -D warnings
CLIPPY_STATUS="${GREEN}CLEAN${NC}"

status "Checking for .env files committed" "${RED}" "🔑"
if git rev-parse --is-inside-work-tree &>/dev/null; then
	if git ls-files --error-unmatch .env 2>/dev/null; then
		echo -e "${RED}\n  ERROR: .env is tracked by git! Add it to .gitignore.\n${NC}"
		report_fail ".env in git"
	else
		report_ok
	fi
fi

status "Checking for unresolved merge markers" "${RED}" "🔍"
if git rev-parse --is-inside-work-tree &>/dev/null; then
	if git grep -l "^<<<<<<< \|^=======$\|^>>>>>>> " -- . 2>/dev/null |
		grep -v node_modules | grep -v target | grep -v ".git/" | grep .; then
		echo -e "${RED}\n  ERROR: Merge conflict markers found in files above.\n${NC}"
		report_fail "merge markers"
	else
		report_ok
	fi
fi

# ══════════════════════════════════════════════════════════════════════════════
# Phase 3: Build
# ══════════════════════════════════════════════════════════════════════════════
if $QUICK_MODE; then
	echo -e "\n${YELLOW}⚠ Skipping build & test phases (--quick)${NC}"
	BUILD_STATUS="${YELLOW}SKIPPED${NC}"
	TEST_STATUS="${YELLOW}SKIPPED${NC}"
	DOC_STATUS="${YELLOW}SKIPPED${NC}"
else
	header "Phase 3: Build"

	status "Building Rust workspace" "${MAGENTA}" "🏗️"
	run_step "cargo build" cargo build --workspace --all-targets --locked
	BUILD_STATUS="${GREEN}CLEAN${NC}"

	status "Building TypeScript frontend" "${MAGENTA}" "🏗️"
	if [ -d "ui" ]; then
		run_step "pnpm build" bash -c 'cd ui && pnpm run build'
		UI_BUILD_STATUS="${GREEN}CLEAN${NC}"
	else
		echo -e " [ ${YELLOW}SKIP${NC} ] ui/ not found"
		UI_BUILD_STATUS="${YELLOW}SKIPPED${NC}"
	fi

	# ══════════════════════════════════════════════════════════════════════════
	# Phase 4: Tests
	# ══════════════════════════════════════════════════════════════════════════
	header "Phase 4: Tests"

	status "Running Rust unit & integration tests" "${GREEN}" "🧪"
	run_step "cargo test" cargo test --workspace --lib --bins --tests --examples

	status "Running Rust doc tests" "${CYAN}" "📚"
	run_step "cargo test --doc" cargo test --doc
	TEST_STATUS="${GREEN}PASSED${NC}"

	# ══════════════════════════════════════════════════════════════════════════
	# Phase 5: Documentation
	# ══════════════════════════════════════════════════════════════════════════
	header "Phase 5: Documentation"

	status "Building Rust API docs" "${MAGENTA}" "📖"
	run_step "cargo doc" cargo doc --workspace --no-deps --document-private-items
	DOC_STATUS="${GREEN}GENERATED${NC}"
fi

# ══════════════════════════════════════════════════════════════════════════════
# Summary Dashboard
# ══════════════════════════════════════════════════════════════════════════════
STAMP_END=$(date +%s)
DURATION=$((STAMP_END - STAMP_START))

DOC_URI="file://${WORKSPACE_ROOT}/target/doc/velo_core/index.html"
if [[ -t 1 ]]; then
	DOC_LINK="\e]8;;${DOC_URI}\a${CYAN}${BOLD}[ Open Rust Docs ↗ ]${NC}\e]8;;\a"
else
	DOC_LINK="${CYAN}${DOC_URI}${NC}"
fi

echo -e "\n${BOLD}${MAGENTA}📊 Velo Quality Dashboard${NC}"
echo -e "${CYAN}────────────────────────────────────────────────────────────────────────────────${NC}"
printf "  %-32s %b\n" "Platform:" "${GREEN}${PLATFORM}${NC}"
printf "  %-32s %b\n" "Workspace:" "${CYAN}${WORKSPACE_ROOT}${NC}"
printf "  %-32s %b\n" "Rust Formatting:" "${FMT_STATUS}"
printf "  %-32s %b\n" "Cargo Check:" "${CARGO_CHECK_STATUS}"
printf "  %-32s %b\n" "TypeScript:" "${TS_STATUS:-${YELLOW}NOT CHECKED${NC}}"
printf "  %-32s %b\n" "Biome Lint:" "${BIOME_STATUS:-${YELLOW}NOT CHECKED${NC}}"
printf "  %-32s %b\n" "Clippy Lints:" "${CLIPPY_STATUS}"
printf "  %-32s %b\n" "Build:" "${BUILD_STATUS:-${YELLOW}SKIPPED${NC}}"
printf "  %-32s %b\n" "UI Build:" "${UI_BUILD_STATUS:-${YELLOW}SKIPPED${NC}}"
printf "  %-32s %b\n" "Tests:" "${TEST_STATUS:-${YELLOW}SKIPPED${NC}}"
printf "  %-32s %b\n" "Documentation:" "${DOC_STATUS:-${YELLOW}SKIPPED${NC}}"
printf "  %-32s %b\n" "Docs URL:" "${DOC_LINK}"
printf "  %-32s %b\n" "Total Audit Time:" "${DURATION} seconds"
echo -e "${CYAN}────────────────────────────────────────────────────────────────────────────────${NC}"
echo -e "\n${BOLD}${GREEN}✅ Velo lint & verification complete.${NC}"
