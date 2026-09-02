---
name: project-live-edit-model
description: "Google-Docs editing model shipped 2026-06-11 — doc is truth, daemon is single writer, SaveBuffer/Saved wire pair; where each piece lives"
metadata: 
  node_type: memory
  type: project
  originSessionId: 970ff237-e4ca-454b-a88c-c03ba599d030
---

Live-edit model (user-decided, shipped 2026-06-11): the daemon's CRDT hub doc is truth, disk is a projection, the daemon is the SINGLE writer. Solo behavior is byte-identical to old save (doc == buffer when alone) — no mode switch between solo and multiplayer.

**Why:** user asked "1) solo = regular save, 2) multi = google-docs live"; the unified model gives both without a mode flag.

**How to apply:**
- Wire pair: `CrdtClientMessage::SaveBuffer` / `CrdtServerMessage::Saved { buffer_id, bytes_written }` (protocol crdt.rs). `Saved` BROADCASTS — dirty bit is doc-level, every client clears it.
- Daemon flush: `CrdtSyncHub::save_buffer` (crdt/sync.rs); only `file://` ids.
- nvim `:w`: BufWriteCmd autocmd attached in `attach_buffer_change_events` (nvim.rs) → `neoism_crdt_write` rpcnotify → same channel as on_lines (ordering guarantees keystrokes precede flush). Channel payload is now `NvimBufferEvent { Lines, WriteRequested }`.
- Desktop Cmd+P-write: `save_current_markdown_via_daemon` (markdown_crdt.rs) when binding seeded; local fs::write only as no-daemon fallback. `Saved` arm runs post-save hooks (tasks-save, note index/graph, toast).
- Web (8D outbound shipped same commit): wasm `crdt_pump(bufferId)` / `crdt_apply(json)` / `markdown_request_save()` on ChromeBridge; TS pumps from the markdown key handler (letter-by-letter) + 250ms presence heartbeat; Ctrl+S in TerminalPanel.handleKeyDown markdown branch. TS owns path→buffer-id (`activeMarkdownBufferId`, same scheme as presence).
- End-to-end test with live nvim: neoism-workspace-daemon/tests/crdt_save.rs.
- Gotcha: wasm free helpers must NOT sit between `#[wasm_bindgen]` and its struct.
- nvim caret rendering gotchas (7C-2, all hit in one day): `Context.editor_path` was declared-but-never-assigned (set from BufferOpened now) — killed desktop publish+paint silently; caret columns need nvim's `getwininfo().textoff` (gutter width — autocmd pushes it, WinViewport carries it, each renderer adds ITS OWN); carets must ride the editor scroll spring's `pixel_offset_y` (yank-flash pattern) or they bounce; the roster reuses md's constants/initials from `editor::markdown::roster` via shared `panels/remote_carets.rs`. Chrome hides console.debug — diagnostics must log at info.
- Co-editing daemon load: CRDT→nvim replays coalesce behind a 40ms quiet window (full-text exec_lua per keystroke × sessions wedged the embedded daemon = frozen desktop); orphan nvim sessions reaped per-connection-namespace on ws close.
- Gotcha (nvim↔nvim went silent): every screen runs its OWN embedded nvim session; stamping all on_lines folds with the one daemon client id made each session's echo guard swallow every other session's edits. Fix: per-session random origin id (`generate_session_origin`, nvim.rs); `apply_nvim_lines_change` takes `origin_client_id`; applier skips only its own. Symptom signature: cursors sync, text doesn't.
- Web tripwire: stale wasm bundle (no `crdt_pump` export) gives the same cursors-but-no-text symptom on markdown — TerminalPanel raises a "hard-refresh" toast via `crdtSupported()`.
- **SPLIT BRAIN (the day's true villain, fixed 2026-06-11)**: desktop-first boot embedded a unix-socket-only daemon while web dialed TCP 7878 → two daemons, nothing crosses, looks like sync bugs. Fix: embedded daemon also binds 127.0.0.1:7878 best-effort (embedded_daemon.rs); standalone SIGTERM now exits immediately instead of axum-graceful-draining websockets forever (main.rs select, no with_graceful_shutdown). User's `pkill neoism` also matches the daemon's comm — assume daemons get SIGTERM'd at every desktop relaunch. Preferred topology now: NO standalone on this machine — the desktop IS the brain (unix + TCP), web connects to it.
- **WASM BUNDLE PATH: the browser loads `neoism-frontend/web/src/wasm/` (loadRealWasm in createTerminal.ts), NOT `public/neoism-terminal-wasm/`** — the README pointed at the stale public/ copy and a day was lost rebuilding into the wrong dir. Build: `wasm-pack build --target web -d neoism-frontend/web/src/wasm neoism-frontend/wasm` (needs RUSTUP_HOME/CARGO_HOME/PATH + RUSTUP_TOOLCHAIN=1.92 env).
- Daemon in-crate lib test target (workspace.rs cfg(test)) is pre-existing-broken (old ManagerInner/OpenWorkspace API); run integration tests via explicit `--test` names.

Related: [[project-cross-laptop-status]], [[project-markdown-editor]]
