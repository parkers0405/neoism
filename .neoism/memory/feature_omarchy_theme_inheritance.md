---
name: "Omarchy theme inheritance"
description: "Native Omarchy theme inheritance reads current colors.toml and hot-reloads atomic theme switches"
type: "feature"
scope: "project"
origin: "Implemented in workspace session"
created: "2026-08-24"
updated: "2026-08-24"
---

Neoism desktop natively imports Omarchy's active semantic palette from `$XDG_STATE_HOME/omarchy/current/theme/colors.toml` (fallback `~/.local/state/...`) as runtime IDE theme `omarchy`. The adapter lives in `neoism-backend/src/config/mashup.rs` and maps Omarchy background/foreground/accent/ANSI roles to all Neoism chrome, terminal, and syntax roles. `neoism-frontend/desktop/src/terminal/watcher.rs` recursively watches Neoism runtime themes/packs plus Omarchy's stable `current` directory, routing atomic theme swaps through the existing debounced config reload. Users opt in once with `appearance.theme: "omarchy"`; thereafter `omarchy-theme-set` applies live without templates, symlinks, or hooks. Desktop-only; web/Wasm custom disk themes remain unsupported. Focused backend adapter tests and watcher tests pass; `cargo check -p neoism` passes.
