# Aethos Client Update Lifecycle

## Objective

Provide a calm, non-disruptive update experience:

- background update check on app launch
- subtle in-app notification when newer release exists
- clear version delta (`current` vs `latest`)
- release notes access in one click
- installer download entry point in one click

Users continue using the app while update information remains available.

## Current implementation (v1)

### 1) Background release check

- On startup, the desktop client performs a delayed, non-blocking check against:
  - `https://api.github.com/repos/natemellendorf/aethos-client/releases/latest`
- Local running version is provided by backend command `app_version`.
  - `app_version` resolves from embedded root `Cargo.toml` release version at build time.
- If latest > local (semver compare), show update notification card.

### 2) In-app notification UX

- Subtle card appears beneath top navigation.
- Includes:
  - running version
  - latest version
  - short release-notes summary
  - `Release Notes` action
  - `Download Update` action (installer asset for current platform when available)
  - `Dismiss`

### 3) Resource handling / resilience

- Update check is best-effort and silent on failure.
- No startup blocking, no modal interruption.
- URLs open via backend `open_external_url` command, restricted to `http(s)`.

## Platform installer mapping (v1)

The client chooses release assets by runtime platform:

- Windows -> `.exe`
- macOS -> `.dmg`
- Linux -> `.AppImage` preferred, fallback `.deb`

## Lifecycle model

1. User launches app.
2. App loads normal UX immediately.
3. Background check compares local and latest release versions.
4. If newer release exists, app shows subtle update card.
5. User can keep working or choose update actions.
6. User downloads installer and applies update.
7. Relaunch app on new version.

## Why this first

This keeps update behavior deterministic and low-risk while preserving current GitHub-first distribution.

## Phase 2 (automatic install + restart)

Target behavior (future):

- `Update now` downloads package, installs, and relaunches app automatically.

Recommended path:

1. Adopt Tauri updater plugin with signed update artifacts.
2. Produce updater metadata (`latest.json`) in release pipeline.
3. Add secure signing key management in CI.
4. Wire in-app `Install update` + `Restart now` flow.

This phase is intentionally separated due signing and packaging security requirements.

## Validation checklist

- [ ] Launch app while on latest -> no update card
- [ ] Launch app while behind -> update card appears
- [ ] Versions shown correctly
- [ ] Release notes button opens browser
- [ ] Download update button opens platform installer asset
- [ ] App remains fully usable while card is shown
