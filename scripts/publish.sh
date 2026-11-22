#!/usr/bin/env bash
#
# scripts/publish.sh — cut a gabion release to crates.io.
#
# SECURITY: this script contains ZERO credentials. There is no token literal here,
# no token in a flag, and no token in any line it prints. It never reads, copies,
# or uploads a token itself: `cargo publish` uses whatever credential cargo
# already has (CARGO_REGISTRY_TOKEN / CARGO_REGISTRY_CREDENTIALS in the
# environment, or $CARGO_HOME/credentials.toml from `cargo login`). The script
# only checks that *some* credential exists, reports where it came from, and
# redacts anything shaped like a token in case a tool spews the environment.
#
# Usage:
#   scripts/publish.sh                 # full release: gate, publish, tag, push tag
#   scripts/publish.sh --dry-run       # everything except uploads and the tag push
#   scripts/publish.sh --yes           # no interactive confirmation
#   scripts/publish.sh --skip-check    # skip `gabion check` (only if CI is already green)
#   scripts/publish.sh --no-tag        # publish, but do not create/push the tag
#   scripts/publish.sh -m "prose"      # tag annotation (default: the release commit message)
#
# Re-running is safe: a crate already at <version> on the registry is skipped, and
# an existing tag is left alone.
#
set -euo pipefail

# mapfile, ${var,,} and empty-array handling under set -u all need bash 4+.
# macOS ships 3.2 at /bin/bash; use `brew install bash` if this trips.
if [[ "${BASH_VERSINFO[0]:-0}" -lt 4 ]]; then
  echo "publish.sh needs bash 4+ (found ${BASH_VERSION:-unknown}); run it with a newer bash" >&2
  exit 1
fi

# --- the publish set -----------------------------------------------------------
# Publish order, dependencies first. gabion-macros has no sibling deps; gabion
# depends on gabion-macros; gabion-native and gabion-web depend on gabion;
# gabion-cli depends on none of them. The set itself is derived from the manifests
# below — this list only fixes the order, and the script fails if the two disagree,
# so adding a sixth crate means editing one array, not hunting through the logic.
ORDER=(gabion-macros gabion gabion-native gabion-web gabion-cli)

CRATES_DIR="crates"
INDEX="https://index.crates.io"
UA="gabion-publish-script (https://github.com/d-buckner/gabion)"
GATE=(cargo run -p gabion-cli -- check)

# --- flags ---------------------------------------------------------------------
DRY_RUN=0
ASSUME_YES=0
SKIP_CHECK=0
NO_TAG=0
TAG_MESSAGE=""
VERSION_OVERRIDE=""

usage() {
  # Print the header comment block (everything between the shebang and `set -euo`),
  # so --help can never drift from the comment at the top of the file.
  awk 'NR > 1 { if ($0 !~ /^#/) exit; sub(/^# ?/, ""); print }' "$0"
  exit "${1:-0}"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    -n|--dry-run)    DRY_RUN=1; shift ;;
    -y|--yes)        ASSUME_YES=1; shift ;;
    --skip-check)    SKIP_CHECK=1; shift ;;
    --no-tag)        NO_TAG=1; shift ;;
    -m|--message)    TAG_MESSAGE="${2:?-m needs a value}"; shift 2 ;;
    --version)       VERSION_OVERRIDE="${2:?--version needs a value}"; shift 2 ;;
    -h|--help)       usage 0 ;;
    *) echo "unknown flag: $1 (see --help)" >&2; usage 2 ;;
  esac
done

# --- output --------------------------------------------------------------------
if [[ -t 1 ]]; then
  C_DIM=$'\033[2m'; C_BOLD=$'\033[1m'; C_RED=$'\033[31m'; C_GRN=$'\033[32m'; C_YEL=$'\033[33m'; C_OFF=$'\033[0m'
else
  C_DIM=""; C_BOLD=""; C_RED=""; C_GRN=""; C_YEL=""; C_OFF=""
fi

step() { printf '\n%s==> %s%s\n' "$C_BOLD" "$*" "$C_OFF"; }
info() { printf '    %s%s%s\n' "$C_DIM" "$*" "$C_OFF"; }
ok()   { printf '    %s✓%s %s\n' "$C_GRN" "$C_OFF" "$*"; }
warn() { printf '    %s!%s %s\n' "$C_YEL" "$C_OFF" "$*" >&2; }
die()  { printf '\n%s✗ %s%s\n' "$C_RED" "$*" "$C_OFF" >&2; exit 1; }

# Anything that smells like a credential never reaches the terminal or a log file.
redact() {
  sed -E 's/(crates_io_|cio[A-Za-z0-9]{8,}|tokn[a-z0-9]{21,}|x-access-token)/[REDACTED]/g'
}
run() { "$@" 2>&1 | redact; return "${PIPESTATUS[0]}"; }

# --- self-audit: prove the zero-token claim ------------------------------------
# Cheap, and it fails the moment someone "helpfully" pastes a token into this file.
self_audit() {
  local hits
  hits=$(grep -nE '(crates_io_[A-Za-z0-9]{10,}|tokn[a-z0-9]{21,})' "$0" | redact || true)
  if [[ -n "$hits" ]]; then
    die "self-audit found a credential-shaped literal in $0 — remove it and use cargo login"
  fi
}

# --- registry lookups (no auth, no downloads) ----------------------------------
# The sparse index stores one JSON line per version at a bucket derived from the
# name length. Cheaper than `cargo info`, which downloads the crate.
index_path() {
  local n="${1,,}"
  case ${#n} in
    1) echo "1/${n}" ;;
    2) echo "2/${n}" ;;
    3) echo "3/${n}" ;;
    *) echo "${n:0:2}/${n:2:2}/${n}" ;;
  esac
}

registry_versions() {
  curl -fsS --max-time 20 -H "User-Agent: ${UA}" "${INDEX}/$(index_path "$1")" \
    | grep -o '"vers":"[^"]*"' | cut -d'"' -f4 || true
}

is_published() {
  grep -qx -- "$2" <(registry_versions "$1" | tr ' ' '\n')
}

# --- credential presence, without touching the value ---------------------------
# Reports the *source* only. Never prints the token, its length, or a prefix.
check_auth() {
  local source=""
  if [[ -n "${CARGO_REGISTRY_TOKEN:-}" || -n "${CARGO_REGISTRY_CREDENTIALS:-}" ]]; then
    source="environment (CARGO_REGISTRY_TOKEN / CARGO_REGISTRY_CREDENTIALS)"
  else
    local creds="${CARGO_HOME:-$HOME/.cargo}/credentials.toml"
    if [[ -f "$creds" ]] && grep -qE '^[[:space:]]*(token|secret|registry\.crates-io\.token)[[:space:]]*=' "$creds"; then
      source="cargo config ($creds)"
      local mode
      mode=$(stat -f '%Lp' "$creds" 2>/dev/null || stat -c '%a' "$creds" 2>/dev/null || echo "")
      if [[ -n "$mode" && "${mode: -1}" != "0" ]]; then
        warn "credentials file is group/world-readable (mode $mode); run: chmod 600 $creds"
      fi
    fi
  fi
  [[ -n "$source" ]] || die "no cargo credential found. Run 'cargo login' or set CARGO_REGISTRY_TOKEN.
    This script will never store it: cargo reads it itself."
  ok "credential present: ${source}"
  info "the token value is never read or printed by this script"
}

# --- workspace introspection ---------------------------------------------------
workspace_version() {
  awk -F'"' '/^\[workspace\.package\]/{f=1; next} f && /^\[/{f=0} f && /^version[[:space:]]*=/{print $2; exit}' Cargo.toml
}

# A crate is published iff it inherits the workspace version and does not say
# `publish = false`. That is exactly the five crates, and it keeps the example
# hosts out of the set without a hardcoded exclusion list.
published_crates() {
  local manifest
  for manifest in "${CRATES_DIR}"/*/Cargo.toml; do
    [[ -f "$manifest" ]] || continue
    grep -q '^version\.workspace[[:space:]]*=[[:space:]]*true' "$manifest" || continue
    grep -q '^publish[[:space:]]*=[[:space:]]*false' "$manifest" && continue
    basename "$(dirname "$manifest")"
  done
}

# --- main ----------------------------------------------------------------------
command -v cargo >/dev/null || die "cargo not on PATH"
command -v git   >/dev/null || die "git not on PATH"
command -v curl  >/dev/null || die "curl not on PATH (needed for registry checks)"

self_audit

cd "$(git rev-parse --show-toplevel)" || die "not inside a git repository"

step "Preflight"
[[ -z "$(git status --porcelain)" ]] || die "working tree is dirty; commit or stash first"
ok "working tree clean"

BRANCH="$(git rev-parse --abbrev-ref HEAD)"
if [[ "$BRANCH" != "main" && "$BRANCH" != "master" ]]; then
  die "on '$BRANCH'; release from main after the release PR merges"
fi
ok "on $BRANCH"

git fetch --quiet origin "$BRANCH"
if [[ "$(git rev-parse HEAD)" != "$(git rev-parse "origin/$BRANCH")" ]]; then
  die "HEAD ($(git rev-parse --short HEAD)) is not origin/$BRANCH ($(git rev-parse --short "origin/$BRANCH")); push first"
fi
ok "HEAD is pushed to origin/$BRANCH ($(git rev-parse --short HEAD))"

VERSION="${VERSION_OVERRIDE:-$(workspace_version)}"
[[ -n "$VERSION" ]] || die "could not read [workspace.package] version from Cargo.toml"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-.][A-Za-z0-9.]+)?$ ]] || die "malformed version: $VERSION"
ok "version ${C_BOLD}${VERSION}${C_OFF} (from [workspace.package])"

# The set from the manifests must match the ORDER array exactly, in either direction.
mapfile -t FOUND < <(published_crates) || true
[[ ${#FOUND[@]} -gt 0 ]] || die "no publishable crates found under ${CRATES_DIR}/"
for c in "${ORDER[@]}"; do
  printf '%s\n' "${FOUND[@]}" | grep -qx "$c" || die "'$c' is in ORDER but not a publishable crate (renamed? publish=false?)"
done
for c in "${FOUND[@]}"; do
  printf '%s\n' "${ORDER[@]}" | grep -qx "$c" || die "'$c' is publishable but missing from ORDER; add it in dependency order"
done
ok "publish set matches ORDER: ${ORDER[*]}"

# Every crate must inherit the version, or the release is a lie.
for c in "${ORDER[@]}"; do
  grep -q '^version\.workspace[[:space:]]*=[[:space:]]*true' "${CRATES_DIR}/${c}/Cargo.toml" \
    || die "${c} pins its own version; it must use version.workspace = true"
done
info "all five inherit [workspace.package] version — one line moves them all"

check_auth

# --- what will actually happen -------------------------------------------------
step "Registry state"
declare -a TO_PUBLISH=()
for c in "${ORDER[@]}"; do
  if is_published "$c" "$VERSION"; then
    ok "${c} ${VERSION} already on crates.io — skipping"
  else
    TO_PUBLISH+=("$c")
    LATEST="$(registry_versions "$c" | sort -V | tail -1)"
    printf '    %s→%s %s %s needs publishing %s(latest on registry: %s)%s\n' \
      "$C_DIM" "$C_OFF" "$c" "$VERSION" "$C_DIM" "${LATEST:-none}" "$C_OFF"
    # Publishing a lower version than what is already out would silently downgrade
    # anyone using a bare `version = "0.1"` requirement resolver. LATEST is newer
    # than VERSION only when LATEST sorts above VERSION and is not the same string.
    if [[ -n "$LATEST" && "$LATEST" != "$VERSION" ]]; then
      HIGHER="$(printf '%s\n%s\n' "$LATEST" "$VERSION" | sort -V | tail -1)"
      if [[ "$HIGHER" == "$LATEST" ]]; then
        die "${c} already has ${LATEST} on crates.io, which is newer than ${VERSION}; refusing to downgrade"
      fi
    fi
  fi
done
if [[ ${#TO_PUBLISH[@]} -eq 0 ]]; then
  ok "every crate is already at ${VERSION}"
fi

TAG="v${VERSION}"
if git rev-parse -q --verify "refs/tags/${TAG}" >/dev/null; then
  info "tag ${TAG} already exists locally — will not recreate or move it"
elif git ls-remote --exit-code --tags origin "refs/tags/${TAG}" >/dev/null 2>&1; then
  info "tag ${TAG} already exists on origin — will not push it again"
else
  info "will create annotated tag ${TAG} on $(git rev-parse --short HEAD)"
fi

if [[ $DRY_RUN -eq 1 ]]; then
  step "Dry run — nothing was uploaded"
  info "would publish: ${TO_PUBLISH[*]:-nothing}"
  if [[ $NO_TAG -eq 1 ]]; then
    info "would tag:     skipped (--no-tag)"
  else
    info "would tag:     ${TAG}"
  fi
  exit 0
fi

if [[ ${#TO_PUBLISH[@]} -eq 0 && $NO_TAG -eq 0 ]] && git rev-parse -q --verify "refs/tags/${TAG}" >/dev/null; then
  ok "nothing to do: crates published and tag exists"
  exit 0
fi

if [[ $ASSUME_YES -ne 1 ]]; then
  printf '\n%sPublish %s at %s to crates.io? This is irreversible — you can yank, never delete. [y/N] %s' \
    "$C_BOLD" "${TO_PUBLISH[*]:-nothing}" "$VERSION" "$C_OFF" > /dev/tty
  read -r REPLY < /dev/tty
  [[ "$REPLY" == "y" || "$REPLY" == "Y" ]] || die "aborted by user"
fi

# --- the gate ------------------------------------------------------------------
if [[ $SKIP_CHECK -eq 1 ]]; then
  step "Gate skipped (--skip-check)"
  warn "publishing without a local 'gabion check'; rely on CI having been green for $(git rev-parse --short HEAD)"
else
  step "Gate: gabion check (15 steps — this is what CI runs)"
  run "${GATE[@]}" >/dev/null || die "gabion check failed; fix it before publishing"
  ok "gabion check: PASS"
fi

# --- package, then publish, dependencies first ---------------------------------
if [[ ${#TO_PUBLISH[@]} -gt 0 ]]; then
  step "Package check"
  for c in "${TO_PUBLISH[@]}"; do
    run cargo package -p "$c" --no-verify >/dev/null || die "cargo package failed for $c"
    ok "${c} packages cleanly"
  done

  step "Publish (dependencies first)"
  for c in "${TO_PUBLISH[@]}"; do
    printf '\n    %s── %s %s%s\n' "$C_BOLD" "$c" "$VERSION" "$C_OFF"
    run cargo publish -p "$c" || die "cargo publish failed for $c.
    If it uploaded but the verify or wait step failed, check 'cargo info ${c}' from
    outside the workspace before retrying — a second publish of the same version
    will be rejected, and this script will then skip it."
    ok "${c} ${VERSION} published"
  done

  step "Confirm the registry agrees"
  for c in "${TO_PUBLISH[@]}"; do
    for _ in 1 2 3 4 5 6 7 8 9 10; do
      is_published "$c" "$VERSION" && break
      sleep 3
    done
    is_published "$c" "$VERSION" || die "${c} ${VERSION} not visible on the sparse index yet; re-run this script in a minute"
    ok "${c} ${VERSION} visible on the sparse index"
  done
fi

# --- tag -----------------------------------------------------------------------
if [[ $NO_TAG -eq 1 ]]; then
  step "Tag skipped (--no-tag)"
elif git rev-parse -q --verify "refs/tags/${TAG}" >/dev/null || git ls-remote --exit-code --tags origin "refs/tags/${TAG}" >/dev/null 2>&1; then
  step "Tag ${TAG} already exists — left alone"
else
  step "Tag ${TAG}"
  # Convention: annotated tag on the release merge commit, message = the release
  # commit's own message when it is the release commit, else a plain subject.
  if [[ -n "$TAG_MESSAGE" ]]; then
    ANNOT="$TAG_MESSAGE"
  elif [[ "$(git log -1 --format=%s)" == "Release ${VERSION}" ]]; then
    ANNOT="$(git log -1 --format=%B)"
    info "annotation taken from the release commit message"
  else
    ANNOT="Release ${VERSION}"
    info "no release commit message found; using a bare subject (pass -m for prose)"
  fi
  git tag -a "$TAG" -m "$ANNOT"
  ok "created annotated tag ${TAG} on $(git rev-parse --short HEAD)"
  run git push origin "$TAG" >/dev/null || die "tag created locally but push failed; push it with: git push origin ${TAG}"
  ok "pushed ${TAG} to origin"
fi

step "Done"
printf '    %s%s%s published to crates.io and tagged\n' "$C_GRN" "$VERSION" "$C_OFF"
for c in "${ORDER[@]}"; do
  printf '      %-14s https://crates.io/crates/%s/%s\n' "$c" "$c" "$VERSION"
done
printf '\n    %sFollow-ups: close the release milestone, and post release notes if the repo uses them.%s\n' "$C_DIM" "$C_OFF"
