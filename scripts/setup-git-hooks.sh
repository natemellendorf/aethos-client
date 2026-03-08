#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HOOK_SOURCE_DIR="${ROOT_DIR}/scripts/hooks"
HOOK_TARGET_DIR="${ROOT_DIR}/.git/hooks"

log() {
  printf '[aethos-hooks] %s\n' "$*"
}

[[ -d "${ROOT_DIR}/.git" ]] || {
  printf '[aethos-hooks] ERROR: %s is not a git repository root\n' "${ROOT_DIR}" >&2
  exit 1
}

mkdir -p "${HOOK_TARGET_DIR}"

for hook in pre-commit pre-merge-commit pre-push; do
  src="${HOOK_SOURCE_DIR}/${hook}"
  dst="${HOOK_TARGET_DIR}/${hook}"
  [[ -f "${src}" ]] || {
    printf '[aethos-hooks] ERROR: missing hook source %s\n' "${src}" >&2
    exit 1
  }
  cp "${src}" "${dst}"
  chmod +x "${dst}"
  log "installed ${hook}"
done

chmod +x "${ROOT_DIR}/scripts/release/create-prerelease.sh"
chmod +x "${ROOT_DIR}/scripts/release/create-release.sh"

log "git hooks installed successfully"
