---
name: "Rust Analyzer warm-up UI slowdown — FIXED"
description: "Zed-style document-scoped diagnostics, sticky anchors, and background git analysis fix RA warm-up frame starvation"
type: "bug"
scope: "project"
origin: "session"
created: "2026-09-02"
updated: "2026-09-02"
---

# Rust Analyzer warm-up UI slowdown — fixed with Zed-style pipeline

Diagnosed and fixed 2026-09-02. Live Neoism-owned rust-analyzer for the 32-crate workspace reached ~6.15 GB RSS plus two proc-macro workers (~440 MB). There was one analyzer, directly parented by desktop, no orphan duplication, no swap, and zero system pressure. UI recovered when initial indexing settled.

Root Neoism amplification:
- lsp_client recursively normalized every server notification under documents mutex, including high-volume `$/progress`/logs that are discarded.
- Desktop used process-global DIAG_VERSION polling; any file publish could make focused pane refold diagnostics during render.
- Coordinates were already normalized to UTF-8 bytes by engine but desktop treated them as UTF-16 again.
- First-window-captured singleton wake did not correctly fan multi-window updates.
- Anchor diagnostic derivation and synchronous git mark/whole-buffer size work occurred in render-called pump.

Final architecture, modeled on Zed:
- Only publishDiagnostics notifications undergo notification coordinate normalization; responses normalize in request waiter.
- Diagnostics worker maps complete engine merged snapshots once to zero-based UTF-8 byte ranges.
- Newest-wins mailbox keyed by canonical `(root,file)` with coalesced wake.
- `CodeDiagnosticsReady` application event drains once and fans to every matching local pane across all windows/grids; unrelated panes do no work.
- Seeded documents pin diagnostics to byte-coordinate sticky CRDT anchors; UTF-8-safe projection handles multiline, boundaries, zero-width, and empty clears.
- Global DIAG_VERSION/store/publish-seq polling and heuristic line reanchor removed.
- Sticky-anchor re-resolution occurs in post-CRDT/editor service turn, not render.
- Git baseline loading, 512KB scan, and compute_git_marks run on one bounded process-global newest-wins worker keyed by window/route/path and revision; stale results rejected; remote panes excluded.
- `pump_code_lsp` retains revision-triggered didChange serialization/query timers, but no diagnostic/git analytical work.

Files include agent-server lsp_client, backend RioEvent, desktop app and code lsp bridge, shared code pane types/helpers.

Verification: 7 diagnostics/git queue tests pass; cargo check for neoism-agent-server, neoism, and neoism-workspace-daemon passes; git diff --check passes. Pre-existing warnings only.
