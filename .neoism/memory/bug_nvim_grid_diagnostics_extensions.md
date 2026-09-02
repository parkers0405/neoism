---
name: "Nvim grid stale rows and diagnostic flicker fixes"
description: "Root causes and fixes for duplicate editor rows, disappearing inline diagnostics, and unstable Godot diagnostic counts."
type: "bug"
scope: "project"
origin: "agent"
created: "2026-07-09"
updated: "2026-07-09"
---

---
name: Nvim grid stale rows and diagnostic flicker fixes
description: Root causes and fixes for duplicate editor rows, disappearing inline diagnostics, and unstable Godot diagnostic counts.
type: bug
created: 2026-07-09
updated: 2026-07-09
origin: agent
---

## Root causes

- `apply_redraw_events` shifted the CPU nvim grid on `grid_scroll` but damaged only newly exposed rows, relying on an optional retained-GPU row copy. When scrollback and animation offsets cancelled, the copy did not run and unchanged GPU rows stayed stale until cursor damage touched them.
- LSP `publishDiagnostics` from each server overwrote one file-level cache entry and was forwarded as a complete snapshot. Multi-publisher setups such as Godot therefore oscillated counts and inline items between partial or empty server payloads.
- Extension catalog included GitHub-release packages with no asset for the current platform, and GUI-launched macOS installers could miss Homebrew paths.

## Fixes

- Damage every row whose CPU cells moved after nvim `grid_scroll`; retained copies remain optimization-only.
- Key LSP diagnostics by `(file, language/server)` and merge all publishers before broadcasting or serving cached diagnostics.
- Filter manifests unsupported on the current host from extension catalog rows.
- Add `/opt/homebrew/bin` and `/usr/local/bin` to subprocess PATH on macOS for npm/python installer commands.

## Verification

- `cargo check -p neoism-backend -p neoism-extensions -p neoism --lib`
- targeted backend grid-scroll test
- targeted extension unsupported-host test
- targeted multi-server diagnostic merge test
