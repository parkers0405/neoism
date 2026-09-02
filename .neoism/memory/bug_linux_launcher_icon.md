---
name: "Linux launcher icon divergence"
description: "Linux launcher icon unified with canonical Neoism app artwork across bootstrap, Nix, DEB, and RPM"
type: "bug"
scope: "project"
origin: "User report and implementation on 2026-07-26"
created: "2026-07-26"
updated: "2026-07-26"
---

## Root cause

Linux bootstrap installed two assets with the same Freedesktop icon name `neoism`:

- canonical square `512x512/apps/neoism.png`
- splash wordmark `scalable/apps/neoism.svg`

Linux launchers prefer the scalable icon, so the wordmark masked the canonical app art. DEB/RPM GoReleaser config also retained stale `misc/rio.desktop` / `misc/logo.svg` paths, and Nix expected nonexistent `misc/logo.svg`.

## Fix

- Linux bootstrap installs/updates only `neoism-frontend/desktop/assets/icons/neoism.png`.
- Bootstrap removes the old `~/.local/share/icons/hicolor/scalable/apps/neoism.svg` migration artifact and overwrites a mismatched cached PNG, then refreshes icon/desktop caches.
- Nix installs the same PNG at `share/icons/hicolor/512x512/apps/neoism.png`.
- Every GoReleaser DEB/RPM architecture uses `misc/neoism.desktop` and the same PNG under the Neoism icon name.
- Keep `Icon=neoism` in the desktop entry so Freedesktop theme resolution works.

## Canonical asset

`neoism-frontend/desktop/assets/icons/neoism.png`: 512x512 RGBA. It is also byte-identical to the web 512 icon; macOS/Windows use their platform containers.

## Verification

- `cargo check -p neoism --message-format=short`, rustfmt, and diff check passed.
- No stale Rio/logo paths remain in GoReleaser or Nix.
- All package source paths exist.
- GoReleaser executable was not installed locally, so `goreleaser check` could not run.
