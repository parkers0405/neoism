---
name: "Notes vault selector required project workspace"
description: "Notes vault actions no longer require active project metadata on Linux or macOS"
type: "bug"
scope: "project"
origin: "User report and implementation on 2026-07-26"
created: "2026-07-26"
updated: "2026-07-26"
---

## Root cause

Notes vault selection, add, and rename still loaded the active project's `.neoism/workspace.json` and warned `No active Neoism workspace` when none existed. This contradicted the current model: vaults are global objects under `~/Neoism/Vaults`, and the notes sidebar tracks the vault being VIEWED independently from any project's linked vault.

## Fix

In `screen/bridges/workspace/vault_ops.rs`:

- `switch_notes_vault` always opens/ensures `vault_notes_workspace(name)` directly. It no longer loads, rewrites, or saves project metadata.
- `add_notes_vault` creates and views the global vault directly. It no longer requires an active project workspace.
- `rename_notes_vault` renames the sidebar's currently viewed local vault directory directly. It rejects shared/external roots and an already-existing destination instead of touching project config.
- Explicit project-link actions remain project-scoped and are the only notes actions that should mutate `config.notes.workspace`.

This is cross-platform; the report reproduced on macOS and Linux.

## Verification

- `cargo check -p neoism --message-format=short` passed.
- `cargo test -p neoism-workspace-index config::tests::default_notes_workspace_has_stable_global_identity --lib` passed.
- rustfmt and diff check passed.
