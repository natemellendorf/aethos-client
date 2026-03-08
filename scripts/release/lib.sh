#!/usr/bin/env bash
set -euo pipefail

repo_root() {
  git rev-parse --show-toplevel
}

last_release_tag() {
  git tag --list 'v[0-9]*.[0-9]*.[0-9]*' --sort=-v:refname | head -n 1
}

current_version() {
  local cargo_toml
  cargo_toml="$(repo_root)/Cargo.toml"
  awk -F ' = ' '/^version = "/ {gsub(/"/, "", $2); print $2; exit}' "${cargo_toml}"
}

semver_bump_level() {
  local last_tag range changelog
  last_tag="$(last_release_tag || true)"

  if [[ -n "${last_tag}" ]]; then
    range="${last_tag}..HEAD"
  else
    range="HEAD"
  fi

  changelog="$(git log ${range} --format='%s%n%b%n---END---' || true)"

  if [[ -z "${changelog}" ]]; then
    printf 'none\n'
    return
  fi

  if grep -Eq '(^[^[:space:]:]+(\([^)]+\))?!:)|BREAKING CHANGE:' <<<"${changelog}"; then
    printf 'major\n'
    return
  fi

  if grep -Eq '^feat(\([^)]+\))?:' <<<"${changelog}"; then
    printf 'minor\n'
    return
  fi

  printf 'patch\n'
}

next_version() {
  local current bump major minor patch
  current="$(current_version)"
  bump="$(semver_bump_level)"

  IFS='.' read -r major minor patch <<<"${current}"

  case "${bump}" in
    major)
      major=$((major + 1))
      minor=0
      patch=0
      ;;
    minor)
      minor=$((minor + 1))
      patch=0
      ;;
    patch)
      patch=$((patch + 1))
      ;;
    none)
      printf '%s\n' "${current}"
      return
      ;;
    *)
      printf 'unknown bump level: %s\n' "${bump}" >&2
      exit 1
      ;;
  esac

  printf '%s.%s.%s\n' "${major}" "${minor}" "${patch}"
}

release_notes_since_last_tag() {
  local last_tag range
  last_tag="$(last_release_tag || true)"

  if [[ -n "${last_tag}" ]]; then
    range="${last_tag}..HEAD"
  else
    range="HEAD"
  fi

  git log ${range} --pretty='- %s (%h)'
}
