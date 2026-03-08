#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "${SCRIPT_DIR}/lib.sh"

DRY_RUN="0"

usage() {
  cat <<'EOF'
Create an official Aethos Linux release.

Usage:
  scripts/release/create-release.sh [--dry-run]

Behavior:
  - requires clean git state on main
  - infers next semver from conventional commits since last v* tag
  - updates Cargo.toml version
  - runs cargo test
  - commits + tags release
  - creates GitHub release with generated notes
EOF
}

log() {
  printf '[aethos-release] %s\n' "$*"
}

fail() {
  printf '[aethos-release] ERROR: %s\n' "$*" >&2
  exit 1
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run)
      DRY_RUN="1"
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      fail "unknown option: $1"
      ;;
  esac
done

command -v gh >/dev/null 2>&1 || fail "gh CLI is required"

cd "$(repo_root)"

[[ "$(git rev-parse --abbrev-ref HEAD)" == "main" ]] || fail "releases must run from main"
[[ -z "$(git status --porcelain)" ]] || fail "working tree must be clean"

current="$(current_version)"
next="$(next_version)"

if [[ "${current}" == "${next}" ]]; then
  fail "no version bump inferred from commits; nothing to release"
fi

tag="v${next}"
if git rev-parse "${tag}" >/dev/null 2>&1; then
  fail "tag ${tag} already exists locally"
fi

if gh release view "${tag}" >/dev/null 2>&1; then
  fail "release ${tag} already exists on GitHub"
fi

log "current version: ${current}"
log "next version:    ${next}"

if [[ "${DRY_RUN}" == "1" ]]; then
  log "dry run complete"
  exit 0
fi

perl -0777 -i -pe 's/^version = "[0-9]+\.[0-9]+\.[0-9]+"/version = "'"${next}"'"/m' Cargo.toml

cargo test

git add Cargo.toml
git commit -m "chore(release): ${tag}"
git tag -a "${tag}" -m "Release ${tag}"

gh release create "${tag}" \
  --title "Aethos Linux ${tag}" \
  --generate-notes

log "release created: ${tag}"
log "next step: git push origin main --follow-tags"
