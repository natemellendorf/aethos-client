#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "${SCRIPT_DIR}/lib.sh"

log() {
  printf '[aethos-release] %s\n' "$*"
}

fail() {
  printf '[aethos-release] ERROR: %s\n' "$*" >&2
  exit 1
}

command -v gh >/dev/null 2>&1 || fail "gh CLI is required for prerelease creation"

cd "$(repo_root)"

short_sha="$(git rev-parse --short HEAD)"
next="$(next_version)"
stamp="$(date -u +%Y%m%d%H%M%S)"
tag="v${next}-pre.${stamp}.${short_sha}"

if gh release view "${tag}" >/dev/null 2>&1; then
  log "prerelease ${tag} already exists; skipping"
  exit 0
fi

notes="$(release_notes_since_last_tag || true)"
if [[ -z "${notes}" ]]; then
  notes="- No changes detected since last release tag."
fi

gh release create "${tag}" \
  --prerelease \
  --target main \
  --title "Aethos Linux pre-release ${tag}" \
  --notes "${notes}"

log "created prerelease ${tag}"
