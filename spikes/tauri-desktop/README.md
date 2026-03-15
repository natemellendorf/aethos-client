# Tauri Desktop Spike (Isolated)

This directory is an isolated spike to evaluate Tauri as the cross-platform GUI shell.

It does not modify or replace the current GTK implementation.

## Goals

- Validate Linux/macOS/Windows desktop app packaging flow.
- Validate UX fidelity potential with a modern UI layer.
- Validate Rust backend command integration for app logic.

## Included Spike Scope

- Multi-view UI shell: Onboarding, Chats, Contacts, Settings.
- Local mock contact/thread state in frontend for interaction testing.
- Rust command bridge for diagnostics and mock Wayfarer ID generation.

## Run (Dev)

From this directory:

```bash
npm install
npm run tauri:dev
```

## Build (Desktop Bundle)

```bash
npm run tauri:build
```

## Evaluation Checklist

1. App launches on each platform without manual GTK/Homebrew/MSYS runtime setup.
2. UI remains responsive and usable at desktop and narrow widths.
3. Rust commands return expected data in UI.
4. Generated bundles/installers are straightforward to distribute.

## Notes

- Dev server is a minimal static server (`python3 -m http.server`) to keep the spike simple.
- This spike is intentionally small and non-production.
