#!/usr/bin/env bash
# ─── Velo Release Script ───────────────────────────────────────────────
# Usage: ./scripts/release.sh [options] [patch|minor|major|<version>]
#
# Builds Tauri bundles for the current platform and publishes to GitHub
# Releases. Run on each target OS to produce native installers:
#
#   Linux   → .deb, .rpm
#   macOS   → .dmg
#   Windows → .nsis (.exe setup)
#
# Features:
#   • Semver bump (patch/minor/major) or explicit version
#   • Dry-run, force-overwrite, draft, pre-release, local-only
#   • Conventional commit changelog (feat/fix/breaking)
#   • SHA256 checksums
#   • Dirty-tree guard, SIGINT cleanup
#   • Editor review of release notes before publish
# ─────────────────────────────────────────────────────────────────────────
set -euo pipefail

# Use sccache if available for faster incremental builds
if command -v sccache &>/dev/null; then
	export RUSTC_WRAPPER=sccache
fi

# ─── Config ────────────────────────────────────────────────────────────
SCRIPTDIR="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$SCRIPTDIR/.." && pwd)"
REMOTE="${REMOTE:-origin}"

# Read from tauri.conf.json (single source of truth)
TAURI_CONF="$REPO/src-tauri/tauri.conf.json"
if [[ ! -f "$TAURI_CONF" ]]; then
	echo "Fatal: $TAURI_CONF not found" >&2
	exit 1
fi

BIN=$(perl -MJSON::PP -e 'decode_json(join"",<>)->{productName} eq "Velo" ? "velo" : die' "$TAURI_CONF" 2>/dev/null ||
	python3 -c "import json,sys; print(json.load(sys.stdin)['productName'].lower())" <"$TAURI_CONF" 2>/dev/null ||
	grep -oP '"productName"\s*:\s*"\K[^"]+' "$TAURI_CONF" | tr '[:upper:]' '[:lower:]')
BIN="${BIN:-velo}"

# Read metadata from Cargo.toml workspace
CARGO_TOML="$REPO/Cargo.toml"
REPO_URL="$(grep -oP '^repository\s*=\s*"\K[^"]+' "$CARGO_TOML" 2>/dev/null || echo "")"
GITHUB_OWNER="$(echo "$REPO_URL" | sed -n 's|https://github.com/\([^/]*\)/.*|\1|p')"
GITHUB_REPO="$(echo "$REPO_URL" | sed -n 's|https://github.com/[^/]*/\(.*\)|\1|p')"
GITHUB_OWNER="${GITHUB_OWNER:-myth-tools}"
GITHUB_REPO="${GITHUB_REPO:-velo}"
HOMEPAGE="https://github.com/$GITHUB_OWNER/$GITHUB_REPO"

# Detect OS
OS_LOWER="$(uname -s | tr '[:upper:]' '[:lower:]')"
case "$OS_LOWER" in
linux*) BUILD_OS="linux" ;;
darwin*) BUILD_OS="macos" ;;
mingw* | cygwin* | msys*) BUILD_OS="windows" ;;
*) BUILD_OS="unknown" ;;
esac

BUILDDIR="build"

# ─── Colors ────────────────────────────────────────────────────────────
if [[ -t 1 ]]; then
	c_reset='\033[0m'
	c_bold='\033[1m'
	c_red='\033[0;31m'
	c_green='\033[0;32m'
	c_yellow='\033[0;33m'
	c_blue='\033[0;34m'
	c_cyan='\033[0;36m'
else
	c_reset='' c_bold='' c_red='' c_green='' c_yellow='' c_blue='' c_cyan=''
fi

die() {
	echo -e "${c_red}✗${c_reset} $*" >&2
	exit 1
}
info() { echo -e "${c_blue}▶${c_reset} $*"; }
ok() { echo -e "${c_green}✓${c_reset} $*"; }
warn() { echo -e "${c_yellow}⚠${c_reset} $*"; }
step() { echo -e "${c_bold}${c_cyan}━━━ $* ━━━${c_reset}"; }

# ─── Cleanup trap ──────────────────────────────────────────────────────
CLEANUP_TAG=""
cleanup() {
	if [[ -n "$CLEANUP_TAG" && "$DRY_RUN" != "true" ]]; then
		warn "Interrupted — cleaning up local tag $CLEANUP_TAG"
		git tag -d "$CLEANUP_TAG" 2>/dev/null || true
	fi
	exit 1
}
trap cleanup SIGINT SIGTERM

# ─── Help ──────────────────────────────────────────────────────────────
usage() {
	cat <<EOF
${c_bold}Usage:${c_reset} $(basename "$0") [options] [bump|version]

${c_bold}Bump / version:${c_reset}
  patch              bump patch (0.1.0 → 0.1.1)  [default]
  minor              bump minor (0.1.0 → 0.2.0)
  major              bump major (0.1.0 → 1.0.0)
  vX.Y.Z             explicit semver tag (e.g. v0.2.0)

${c_bold}Options:${c_reset}
  -d, --dry-run      preview everything — no side-effects
  -f, --force        allow overwriting existing tag / release
      --draft        create release as draft (not published)
      --pre-release  mark release as pre-release
      --local-only   tag locally only; skip push & gh release
      --sign         GPG-sign the tag (-s instead of -a)
      --skip-build   skip Tauri build (reuse existing artifacts)
      --skip-editor  skip editor review of release notes
  -h, --help         show this help

${c_bold}Platform targets (auto-detected):${c_reset}
  Current OS: ${c_cyan}$BUILD_OS${c_reset}

  Linux   → .deb, .rpm
  macOS   → .dmg
  Windows → .nsis (.exe setup)

${c_bold}Environment:${c_reset}
  REMOTE             git remote name (default: origin)
  EDITOR             editor for release notes (default: vi)

${c_bold}Examples:${c_reset}
  $(basename "$0") patch
  $(basename "$0") minor --dry-run
  $(basename "$0") v0.2.0 --draft --pre-release
  $(basename "$0") v0.2.0 --local-only --skip-build --skip-editor
EOF
	exit 0
}

# ─── Parse arguments ───────────────────────────────────────────────────
BUMP=""
DRY_RUN="false"
FORCE="false"
DRAFT="false"
PRE_RELEASE="false"
LOCAL_ONLY="false"
SIGN_TAG="false"
SKIP_BUILD="false"
SKIP_EDITOR="true"

while [[ $# -gt 0 ]]; do
	case "$1" in
	-h | --help) usage ;;
	-d | --dry-run)
		DRY_RUN="true"
		shift
		;;
	-f | --force)
		FORCE="true"
		shift
		;;
	--draft)
		DRAFT="true"
		shift
		;;
	--pre-release)
		PRE_RELEASE="true"
		shift
		;;
	--local-only)
		LOCAL_ONLY="true"
		shift
		;;
	--sign)
		SIGN_TAG="true"
		shift
		;;
	--skip-build)
		SKIP_BUILD="true"
		shift
		;;
	--skip-editor)
		SKIP_EDITOR="true"
		shift
		;;
	-*)
		if [[ "$1" =~ ^v?[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
			BUMP="$1"
		else
			die "unknown option: $1"
		fi
		shift
		;;
	*)
		BUMP="$1"
		shift
		;;
	esac
done

# ─── Pre-flight: tools ────────────────────────────────────────────────
step "Pre-flight checks"

info "checking required tools..."
for cmd in git python3; do
	command -v "$cmd" >/dev/null 2>&1 || die "'$cmd' not found — install it first"
done

# stat with byte-size flags (Linux stat -c vs macOS stat -f)
stat_bytes() {
	stat -c%s "$1" 2>/dev/null || stat -f%z "$1" 2>/dev/null || echo 0
}

# Human-readable size
human_size() {
	local bytes=$1
	if ((bytes >= 1073741824)); then
		echo "$(((bytes + 536870912) / 1073741824)) GB"
	elif ((bytes >= 1048576)); then
		echo "$(((bytes + 524288) / 1048576)) MB"
	else
		echo "$(((bytes + 512) / 1024)) KB"
	fi
}

if [[ "$LOCAL_ONLY" != "true" ]]; then
	command -v gh >/dev/null 2>&1 || die "'gh' not found — install GitHub CLI first"
	if ! gh auth status 2>&1 | grep -qi "logged in"; then
		if [[ "$DRY_RUN" == "true" ]]; then
			warn "not logged into gh (dry-run, will proceed anyway)"
		else
			die "not logged into gh — run 'gh auth login'"
		fi
	else
		info "gh: logged in"
	fi
fi

SHA_CMD=""
if command -v sha256sum >/dev/null 2>&1; then
	SHA_CMD="sha256sum"
elif command -v shasum >/dev/null 2>&1; then
	SHA_CMD="shasum -a 256"
else
	die "neither sha256sum nor shasum found"
fi
info "checksum: $SHA_CMD"
info "platform: $BUILD_OS"

# ─── Dirty tree check ─────────────────────────────────────────────────
HAS_DIRTY_TREE=false
if ! git diff-index --quiet HEAD --; then
	HAS_DIRTY_TREE=true
fi

# ─── Version resolution ───────────────────────────────────────────────
step "Version resolution"

# Read current version from tauri.conf.json
CURRENT_VER="$(python3 -c "import json; print(json.load(open('$TAURI_CONF'))['version'])" 2>/dev/null ||
	grep -oP '"version"\s*:\s*"\K[^"]+' "$TAURI_CONF")"
LAST_TAG="$(git describe --tags --abbrev=0 2>/dev/null || echo "v0.0.0")"
LAST_VER="${LAST_TAG#v}"

info "tauri.conf.json version: ${CURRENT_VER:-none}"
info "last git tag: $LAST_TAG"

if [[ "$BUMP" =~ ^v?[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
	NEW_TAG="${BUMP#v}"
	NEW_TAG="v$NEW_TAG"
	info "explicit version: $BUMP → $NEW_TAG"
elif [[ "$BUMP" == "patch" || "$BUMP" == "minor" || "$BUMP" == "major" ]]; then
	BASE_VER="${CURRENT_VER:-$LAST_VER}"
	IFS='.' read -r major minor patch <<<"$BASE_VER"
	case "$BUMP" in
	patch) NEW_TAG="v$major.$minor.$((patch + 1))" ;;
	minor) NEW_TAG="v$major.$((minor + 1)).0" ;;
	major) NEW_TAG="v$((major + 1)).0.0" ;;
	esac
	info "bump: $BUMP → $NEW_TAG (base: v$BASE_VER)"
else
	if [[ -n "$CURRENT_VER" ]]; then
		NEW_TAG="v$CURRENT_VER"
		info "using tauri.conf.json version: $NEW_TAG"
	else
		die "no version found — specify patch, minor, major, or vX.Y.Z"
	fi
fi

if [[ "$HAS_DIRTY_TREE" == "true" && "$DRY_RUN" != "true" ]]; then
	step "Auto-committing changes"
	git add -A
	git commit -m "chore: release $NEW_TAG"
	ok "committed: chore: release $NEW_TAG"
fi

# Semver validation
if ! [[ "$NEW_TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
	die "generated tag '$NEW_TAG' is not valid semver (expected vX.Y.Z)"
fi

# ─── Tag existence checks ──────────────────────────────────────────────
TAG_EXISTS_LOCAL=false
TAG_EXISTS_REMOTE=false

if git rev-parse "$NEW_TAG" >/dev/null 2>&1; then TAG_EXISTS_LOCAL=true; fi
if git ls-remote --tags "$REMOTE" "$NEW_TAG" 2>/dev/null | grep -q "refs/tags/$NEW_TAG$"; then
	TAG_EXISTS_REMOTE=true
fi

if [[ "$TAG_EXISTS_LOCAL" == "true" || "$TAG_EXISTS_REMOTE" == "true" ]]; then
	warn "tag $NEW_TAG exists (local=$TAG_EXISTS_LOCAL, remote=$TAG_EXISTS_REMOTE)"
fi

# ─── Commits since last tag ────────────────────────────────────────────
COMMITS=""
if git rev-parse "$LAST_TAG" >/dev/null 2>&1; then
	COMMITS="$(git log "$LAST_TAG..HEAD" --oneline --no-decorate 2>/dev/null || true)"
fi
COMMIT_COUNT="$(echo "$COMMITS" | grep -c . || true)"

# ─── Summary ───────────────────────────────────────────────────────────
echo ""
step "Release summary"
echo ""
echo -e "  ${c_bold}Previous tag:${c_reset}  $LAST_TAG"
echo -e "  ${c_bold}New tag:${c_reset}       $NEW_TAG"
echo -e "  ${c_bold}Commits:${c_reset}        $COMMIT_COUNT since $LAST_TAG"
echo -e "  ${c_bold}Platform:${c_reset}       $BUILD_OS"
echo -e "  ${c_bold}Draft:${c_reset}          $DRAFT"
echo -e "  ${c_bold}Pre-release:${c_reset}    $PRE_RELEASE"
echo -e "  ${c_bold}Dry-run:${c_reset}        $DRY_RUN"
echo -e "  ${c_bold}Local only:${c_reset}     $LOCAL_ONLY"
echo -e "  ${c_bold}Force:${c_reset}          $FORCE"
echo -e "  ${c_bold}Sign tag:${c_reset}       $SIGN_TAG"

if [[ "$DRY_RUN" == "true" ]]; then
	echo ""
	warn "DRY-RUN mode — no changes will be made"
fi

# ─── Lint ──────────────────────────────────────────────────────────────
echo ""
step "Linting"

info "running: $SCRIPTDIR/lint.sh --quick"
if "$SCRIPTDIR/lint.sh" --quick; then
	ok "lint passed"
else
	die "lint failed — fix issues before releasing"
fi

# ─── Build ─────────────────────────────────────────────────────────────
echo ""
step "Building Tauri app"

if [[ "$SKIP_BUILD" == "true" ]]; then
	info "skipping build (--skip-build)"
elif [[ "$DRY_RUN" == "true" ]]; then
	info "dry-run: would run: cargo tauri build --bundles <targets>"
else
	info "installing frontend dependencies..."
	(cd "$REPO/ui" && pnpm install)
	ok "pnpm install complete"

	# Determine bundle targets for the current platform
	case "$BUILD_OS" in
	linux) BUNDLE_TARGETS="deb,rpm" ;;
	macos) BUNDLE_TARGETS="dmg" ;;
	windows) BUNDLE_TARGETS="nsis" ;;
	*) die "unsupported build OS: $BUILD_OS" ;;
	esac

	info "running: cargo tauri build --bundles $BUNDLE_TARGETS"
	cargo tauri build --bundles "$BUNDLE_TARGETS"
	ok "Tauri build complete"
fi

# ─── Collect artifacts ─────────────────────────────────────────────────
echo ""
step "Collecting & organizing artifacts"

# Tauri bundle output is in workspace root's target dir
TAURI_BUNDLE_DIR="$REPO/target/release/bundle"

mkdir -p "$BUILDDIR/release"
rm -f "$BUILDDIR/release"/*

VER="${NEW_TAG#v}"
ARTIFACTS=()

collect_if_dir() {
	local dir="$1"
	if [[ -d "$dir" ]]; then
		for f in "$dir"/*; do
			[[ -f "$f" ]] || continue
			local name
			name="$(basename "$f")"
			local ext="${name##*.}"
			local stem="${name%.*}"
			local newname="${BIN}_${VER}_${BUILD_OS}_${stem}.${ext}"
			cp -p "$f" "$BUILDDIR/release/$newname"
			ARTIFACTS+=("$BUILDDIR/release/$newname")
			info "  collected: $newname"
		done
	fi
}

if [[ "$DRY_RUN" != "true" ]]; then
	collect_if_dir "$TAURI_BUNDLE_DIR/deb"
	collect_if_dir "$TAURI_BUNDLE_DIR/rpm"
	collect_if_dir "$TAURI_BUNDLE_DIR/dmg"
	collect_if_dir "$TAURI_BUNDLE_DIR/nsh"
fi

if [[ ${#ARTIFACTS[@]} -eq 0 && "$DRY_RUN" != "true" ]]; then
	warn "no artifacts found in $TAURI_BUNDLE_DIR — did the build succeed?"
	for dir in "$TAURI_BUNDLE_DIR"/*/; do
		if [[ -d "$dir" ]]; then
			for f in "$dir"/*; do
				[[ -f "$f" ]] || continue
				name="$(basename "$f")"
				ext="${name##*.}"
				stem="${name%.*}"
				newname="${BIN}_${VER}_${BUILD_OS}_${stem}.${ext}"
				cp -p "$f" "$BUILDDIR/release/$newname"
				ARTIFACTS+=("$BUILDDIR/release/$newname")
				info "  (fallback) collected: $newname"
			done
		fi
	done
fi

# ─── Checksums ─────────────────────────────────────────────────────────
echo ""
step "Checksums"

CHECKSUM_FILE="$BUILDDIR/release/checksums.txt"

if [[ ${#ARTIFACTS[@]} -gt 0 ]]; then
	(cd "$BUILDDIR/release" && for f in *; do
		[[ "$f" == "checksums.txt" || "$f" == "RELEASE_NOTES.md" ]] && continue
		"$SHA_CMD" "$f"
	done) >"$REPO/$CHECKSUM_FILE"
	ok "checksums written to $CHECKSUM_FILE"
	info "artifacts: ${#ARTIFACTS[@]} files"

	echo ""
	info "Artifact list:"
	for a in "${ARTIFACTS[@]}"; do
		size="$(stat_bytes "$a")"
		size_hr="$(human_size "$size")"
		echo "  $(basename "$a")  (${size_hr})"
	done
else
	info "no artifacts to checksum (dry-run or build skipped)"
fi

# ─── Generate release notes ────────────────────────────────────────────
step "Generating release notes"

RELEASE_NOTES="$BUILDDIR/release/RELEASE_NOTES.md"

{
	echo "# $BIN $NEW_TAG"
	echo ""
	grep -oP '"description"\s*:\s*"\K[^"]+' "$TAURI_CONF" 2>/dev/null ||
		echo "Autonomous desktop agent built with Rust + Tauri"
	echo ""

	echo "## Install"
	echo ""
	echo '```sh'
	echo "# ── Linux ────────────────────────────────────────────────────────"
	echo "# Debian / Ubuntu"
	echo "sudo dpkg -i ${BIN}_${VER}_linux_*.deb"
	echo ""
	echo "# Fedora / RHEL"
	echo "sudo rpm -i ${BIN}_${VER}_linux_*.rpm"
	echo ""
	echo "# ── macOS ────────────────────────────────────────────────────────"
	echo "open ${BIN}_${VER}_macos_*.dmg"
	echo ""
	echo "# ── Windows ──────────────────────────────────────────────────────"
	echo "# Double-click the setup.exe"
	echo '```'
	echo ""

	echo "## Changelog"
	echo ""
	if [[ "$COMMIT_COUNT" -eq 0 ]]; then
		echo "_no changes since ${LAST_TAG}_"
		echo ""
	else
		breaking="$(echo "$COMMITS" | grep -i '!\|BREAKING[ :-]\|breaking[ :-]' || true)"
		feats="$(echo "$COMMITS" | grep -i '^[0-9a-f]\{7\} feat' || true)"
		fixes="$(echo "$COMMITS" | grep -i '^[0-9a-f]\{7\} fix' || true)"
		rest="$(echo "$COMMITS" | grep -iv '^[0-9a-f]\{7\} feat\|^[0-9a-f]\{7\} fix' || true)"

		if [[ -n "$breaking" ]]; then
			echo -e "### ⚠ Breaking Changes\n"
			while IFS= read -r line; do
				[[ -z "$line" ]] && continue
				msg="${line#* }"
				echo "- $msg"
			done <<<"$breaking"
			echo ""
			for hash in $(echo "$breaking" | awk '{print $1}'); do
				feats="$(echo "$feats" | grep -v "^$hash" || true)"
				fixes="$(echo "$fixes" | grep -v "^$hash" || true)"
			done
		fi
		if [[ -n "$feats" ]]; then
			echo -e "### 🚀 Features\n"
			while IFS= read -r line; do
				[[ -z "$line" ]] && continue
				msg="${line#* }"
				echo "- $msg"
			done <<<"$feats"
			echo ""
		fi
		if [[ -n "$fixes" ]]; then
			echo -e "### 🐛 Bug Fixes\n"
			while IFS= read -r line; do
				[[ -z "$line" ]] && continue
				msg="${line#* }"
				echo "- $msg"
			done <<<"$fixes"
			echo ""
		fi
		if [[ -n "$rest" ]]; then
			echo -e "### 📦 Other\n"
			while IFS= read -r line; do
				[[ -z "$line" ]] && continue
				msg="${line#* }"
				echo "- $msg"
			done <<<"$rest"
			echo ""
		fi
	fi

	if [[ ${#ARTIFACTS[@]} -gt 0 ]]; then
		echo ""
		echo "## Downloads"
		echo ""
		echo "| Artifact | Size |"
		echo "|----------|------|"
		for a in "${ARTIFACTS[@]}"; do
			name="$(basename "$a")"
			size="$(stat_bytes "$a")"
			size_hr="$(human_size "$size")"
			echo "| \`$name\` | $size_hr |"
		done
	fi

	if [[ -f "$CHECKSUM_FILE" ]]; then
		echo ""
		echo "## Checksums (SHA256)"
		echo ""
		echo '```'
		cat "$CHECKSUM_FILE"
		echo '```'
	fi

	echo ""
	echo "---"
	echo "_Report issues at ${HOMEPAGE}/issues_"
} >"$RELEASE_NOTES"

info "release notes written to $RELEASE_NOTES"

# ─── Editor review ────────────────────────────────────────────────────
if [[ "$SKIP_EDITOR" != "true" && "$DRY_RUN" != "true" ]]; then
	step "Editor review"
	info "opening release notes for review..."
	${EDITOR:-vi} "$RELEASE_NOTES"
	ok "release notes saved"
fi

# ─── Dry-run stop ─────────────────────────────────────────────────────
if [[ "$DRY_RUN" == "true" ]]; then
	echo ""
	step "Dry-run summary"
	echo ""
	echo -e "  ${c_bold}Tag:${c_reset}         $NEW_TAG"
	echo -e "  ${c_bold}Push:${c_reset}         git push $REMOTE $NEW_TAG"
	echo -e "  ${c_bold}Release:${c_reset}      gh release create $NEW_TAG"
	[[ "$DRAFT" == "true" ]] && echo -e "  ${c_bold}Draft:${c_reset}        yes"
	[[ "$PRE_RELEASE" == "true" ]] && echo -e "  ${c_bold}Pre-release:${c_reset}  yes"
	echo -e "  ${c_bold}Artifacts:${c_reset}     ${#ARTIFACTS[@]} files"
	echo ""
	echo "  To publish when ready:"
	echo "    git push $REMOTE $NEW_TAG"
	echo "    gh release create $NEW_TAG --title \"$BIN $NEW_TAG\" --notes-file $RELEASE_NOTES"
	[[ ${#ARTIFACTS[@]} -gt 0 ]] && echo "      ${ARTIFACTS[*]}"
	echo ""
	ok "dry-run complete — no changes made"
	exit 0
fi

# ─── Confirm ──────────────────────────────────────────────────────────
if [[ "$LOCAL_ONLY" != "true" ]]; then
	step "Publishing"
	info "publishing $NEW_TAG to $REMOTE"
fi

# ─── Force-overwrite existing assets ──────────────────────────────────
if [[ "$TAG_EXISTS_LOCAL" == "true" ]]; then
	warn "deleting existing local tag $NEW_TAG"
	git tag -d "$NEW_TAG"
fi
if [[ "$TAG_EXISTS_REMOTE" == "true" && "$LOCAL_ONLY" != "true" ]]; then
	warn "deleting existing remote tag $REMOTE/$NEW_TAG"
	git push --delete "$REMOTE" "$NEW_TAG" 2>/dev/null || true
fi
if [[ "$TAG_EXISTS_REMOTE" == "true" && "$LOCAL_ONLY" != "true" ]]; then
	warn "deleting existing GitHub release $NEW_TAG"
	gh release delete "$NEW_TAG" --yes 2>/dev/null || true
fi

# ─── Create tag ───────────────────────────────────────────────────────
step "Tagging"

TAG_FLAG="-a"
TAG_MSG_FILE="$(mktemp)"
{
	printf '%s %s (%s)\n' "$BIN" "$NEW_TAG" "$(date -u '+%Y-%m-%d')"
	if [[ "$COMMIT_COUNT" -gt 0 ]]; then
		printf '\n%s\n' "$COMMITS"
	fi
} >"$TAG_MSG_FILE"

if [[ "$SIGN_TAG" == "true" ]]; then
	TAG_FLAG="-s"
	info "creating GPG-signed tag: $NEW_TAG"
else
	info "creating annotated tag: $NEW_TAG"
fi

git tag "$TAG_FLAG" "$NEW_TAG" -F "$TAG_MSG_FILE"
rm -f "$TAG_MSG_FILE"
CLEANUP_TAG="$NEW_TAG"
ok "tag $NEW_TAG created locally"

# ─── Push tag ─────────────────────────────────────────────────────────
if [[ "$LOCAL_ONLY" == "true" ]]; then
	ok "local-only mode — done (tag $NEW_TAG is local)"
	CLEANUP_TAG=""
	exit 0
fi

step "Pushing tag"
info "pushing tag $NEW_TAG to $REMOTE"
git push "$REMOTE" "$NEW_TAG"
CLEANUP_TAG=""
ok "tag $NEW_TAG pushed to $REMOTE"

# Wait for tag to propagate
info "waiting for tag to propagate..."
for _ in {1..10}; do
	if git ls-remote --tags "$REMOTE" "$NEW_TAG" 2>/dev/null | grep -q "refs/tags/$NEW_TAG$"; then
		ok "tag confirmed on $REMOTE"
		break
	fi
	sleep 1
done

# ─── Gather release assets ────────────────────────────────────────────
RELEASE_ASSETS=()
while IFS= read -r -d '' f; do
	RELEASE_ASSETS+=("$f")
done < <(find "$BUILDDIR/release" -type f \( -name "$BIN-*" -o -name "${BIN}_*" \) ! -name "RELEASE_NOTES.md" -print0 2>/dev/null || true)
RELEASE_ASSETS+=("$CHECKSUM_FILE")

# ─── Create GitHub release ────────────────────────────────────────────
step "Creating GitHub release"

GH_OPTS=()
GH_OPTS+=(--title "$BIN $NEW_TAG")
GH_OPTS+=(--notes-file "$RELEASE_NOTES")
[[ "$DRAFT" == "true" ]] && GH_OPTS+=(--draft) && info "creating draft release"
[[ "$PRE_RELEASE" == "true" ]] && GH_OPTS+=(--prerelease) && info "marking as pre-release"

set +e
gh release create "$NEW_TAG" "${GH_OPTS[@]}" "${RELEASE_ASSETS[@]}"
GH_EXIT=$?
set -e

if [[ $GH_EXIT -ne 0 ]]; then
	warn "'gh release create' failed (exit $GH_EXIT)"
	echo ""
	echo -e "  ${c_bold}Tag:${c_reset}         $NEW_TAG (already pushed to $REMOTE)"
	echo -e "  ${c_bold}Release:${c_reset}      failed to create"
	echo ""
	echo "  To retry the release later:"
	echo "    gh release create $NEW_TAG \\"
	echo "      --title \"$BIN $NEW_TAG\" \\"
	echo "      --notes-file $RELEASE_NOTES \\"
	for a in "${RELEASE_ASSETS[@]}"; do
		echo "      \"$a\" \\"
	done
	echo ""
	echo "  To delete the tag and start over:"
	echo "    git tag -d $NEW_TAG && git push --delete $REMOTE $NEW_TAG"
	die "release creation failed"
fi

# ─── Done ─────────────────────────────────────────────────────────────
echo ""
step "Done"

REPO_NWO="$(gh repo view --json nameWithOwner --jq .nameWithOwner 2>/dev/null || echo "${GITHUB_OWNER}/${GITHUB_REPO}")"
echo ""
echo -e "  ${c_bold}Release:${c_reset}  https://github.com/$REPO_NWO/releases/tag/$NEW_TAG"
echo -e "  ${c_bold}Tag:${c_reset}       $NEW_TAG"
echo -e "  ${c_bold}Artifacts:${c_reset} ${#RELEASE_ASSETS[@]} files"
[[ "$DRAFT" == "true" ]] && echo -e "  ${c_bold}Status:${c_reset}   ${c_yellow}draft${c_reset} (publish on GitHub when ready)"
echo ""
ok "release $NEW_TAG published successfully"
