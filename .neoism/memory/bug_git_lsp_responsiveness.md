---
name: "Git review and LSP responsiveness fixes"
description: "Fixed live review paint diffing, LSP menu discovery blocking, and ordered Git dispatch stalls; preserves restore CRDT lock, profiling still needed"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-22"
updated: "2026-10-22"
---

# Git review and LSP responsiveness fixes

Read-only audit identified concrete frame/event-thread work; user authorized fixes. Native live Git review now uses `panels/git_diff_view_live.rs`: one reusable bounded worker per view, single in-flight snapshot, revision/path/baseline validation, stale projections shown read-only, redraw stops once current. Wasm drives `editor/code/gitdiff_review.rs::ReviewDiffBuilder` in short cooperative slices; indexing/equality/output copying are bounded to 256-byte chunks, Myers depth stays 256, excerpt positions use cumulative metadata instead of prefix rescans. Baselines are shared `Arc<str>` snapshots, including pointer-fast unchanged checks and adopting equal refreshed snapshots to avoid recurring deep comparisons.

LSP context-menu capability path (`lsp.rs`, `lsp_service.rs`, `lsp_adapters/cache.rs`) is now genuinely cache-only with try_lock, fresh adapter hints and exact file/client identity; unknown means frontend leaves actions enabled. No Cargo metadata, path canonicalization, config loading, SDK discovery, transport liveness or potentially last-Arc shutdown in menu reads.

Daemon modern Git dispatch (`server/socket.rs`) now uses bounded 32-entry request/reply FIFO worker per socket, captured roots, preserved mutation ordering, explicit not-executed errors on overload, and disconnect cleanup. Blame remains independent. IMPORTANT residual: restore still holds CRDT hub operation gate across Git I/O because all-alias clean validation, disk restore and authoritative reconciliation must remain atomic; removing lock alone is unsafe. This fix does not eliminate every CRDT restore-related wait or legacy raw-Git fallback behavior.

Verified native `cargo check --locked -p neoism -p neoism-ui -p neoism-workspace-daemon -p neoism-agent-server --tests` and wasm `cargo check --locked -p neoism-terminal-wasm --target wasm32-unknown-unknown --lib`. Lightweight standalone rustc harness exercised actual production diff source for 582 reconstruction/progress cases plus exact replacement fixture; no application build/relaunch or FPS profiling. Full regression tests were compiled, not executed. Preserve user heavy-build workflow. Other audit items (tree rebuilds, patch replay, staging fan-out, diagnostic scans, LSP queue scaling, untracked polling) were not broadly optimized by this scoped fix.
