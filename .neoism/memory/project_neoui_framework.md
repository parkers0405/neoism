---
name: "NeoUI framework: default-off checkpoint"
description: "Default-off NeoUI checkpoint: retained/component/input foundations, legacy default Settings, desktop external blocker, browser/shared compile passes"
type: "project"
scope: "project"
origin: "feature/neoui checkpoint"
created: "2026-10-25"
updated: "2026-10-25"
---

# NeoUI checkpoint

Work lives on `feature/neoui`, based on snapshot `8541ec5bb`; do not merge main as part of the checkpoint. The `neoui` Cargo feature is default-off, forwarded by desktop and wasm to optional shared dependency `neoui`. Shared Settings selects baseline `legacy_state.rs`/`legacy_view.rs` by default; experimental state/view, renderer/input bridge, native IME/editing and shared clipboard/composition are feature-gated.

Implemented foundations: typed ComponentHost ownership/state/events/cleanup, retained Taffy and measurement caches with MeasurementKey/RuntimeStats and presentation-only updates, hierarchical routing/capture/focus/keybinding APIs, grapheme input/undo/composition/clipboard requests, basic widgets, ScrollState/Fenwick virtual list, owner-scoped bounded event delivery/cancellation and executor interfaces. Experimental Settings uses ComponentHost<String>, owned boolean/toggle children, canonical values, and TextInput. These are foundation APIs and a narrow integration, not GPUI parity or full application adoption.

Sugarloaf shapes/text remain separate passes; arbitrary interleaving is unsupported. No production GPU changes were made. Native/browser accessibility, full shaping/bidi editing, candidate-window geometry, scheduler/executor integration, live platform validation and measured performance remain unfinished. Preserve Rio licensing/attribution.

Checkpoint checks: cargo check -p neoui passed; env -u CI cargo check -p neoism-ui --features neoui passed; default shared compiled during desktop check. Browser wasm --features web and --features web,neoui both passed. Desktop default and opt-in checks blocked in concurrent excluded agent-server external_agent/helpers.rs:46 (missing crate::tool_support::truncate). No new tests or full-suite rerun; historical initial-slice test evidence in docs/neoui-architecture.md is explicitly historical. Do not cargo build --release.

Concurrent agent-server/SDK changes are unrelated and must remain unstaged. Stage NeoUI paths explicitly, never git add -A. Check remote branch HEAD after push. Architecture and actual boundaries are in docs/neoui-architecture.md.
