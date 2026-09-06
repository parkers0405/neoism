---
name: "Windows terminal shell identity + lifecycle fixes"
description: "Shared PowerShell startup hook; actual shell through create/attach/reconnect→desktop/web/wasm; CR pre-metadata; OSC completion guards and shell-aware clear aliases. Checks/tests pass, existing Windows test cfg blockers."
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-09-06"
updated: "2026-09-06"
---

Windows terminal integration fixes implemented (no commit):
- `neoism-terminal-pty/src/shell_integration.rs::powershell_args` owns native PowerShell UTF-16LE/EncodedCommand hook, reused by desktop factories and daemon `sessions::shell_for_create`. Preserves profile loading / configured args, avoids duplicate NoExit, leaves explicit command/file/noninteractive invocations untouched. Unix startup wrappers unchanged.
- Additive optional `PtyCreated.shell` is actual spawned program. Registry stores it; creation, explicit AttachPty (even empty backlog), and reconnect snapshots send metadata before bytes.
- Desktop remote contexts start Unknown, ingest identity using request route or existing session route, and ignore local foreground-shell detection on remote panes. Web ProtocolClient→PtyService→App→TerminalPanel caches metadata per session, restores before replay/focus and after async wasm startup; duplicate attach metadata does not consume pending spawns. ChromeBridge setter controls actual payload and composer label. Queued spawn commands use target program.
- TerminalShellKind::command_payload: known Bash/Zsh LF and Fish autosuggestion-clearing framing retained; PowerShell/Cmd CR. Unknown now uses portable CR to avoid pre-metadata Windows submit race (physical Enter, also accepted by Unix PTY line discipline).
- Shared TerminalShellKind::is_clear_command handles shell-aware clear/cls/Clear-Host, Windows case folding, compound-command exclusion. Shared buffer owns clear-block reset and completion cleanup; wasm/desktop delegate to same classifier.
- buffer.rs prompt guesses are disabled once ANY OSC lifecycle was observed (including D-only integration), so blank or prompt-shaped output cannot finish integrated sleep. Removed stale awaiting_command+150ms completion; finish requires observed run transition or advancing D generation. Reset blank anchors on clear/submit; unintegrated joined-shell fallbacks retained.
Verification: native desktop/daemon cargo check --tests PASS; wasm32 check PASS; cargo xwin Windows desktop/daemon production check (--features wgpu) PASS. Targeted tests PASS: 67 terminal-buffer, 2 native hook, 5 wire compatibility, 10 daemon session, 23 web protocol/tab tests; TypeScript typecheck PASS. Full Windows --tests remains pre-existing broken: desktop Windows platform_key_bindings gated not(test), Unix DaemonEndpoint references in tests, daemon Unix permissions.mode and Unix-only shell-script tests ungated. Plain cargo check Windows needs SDK; use cargo xwin with cached SDK. No real Windows interactive runtime validation in this task.
Did not edit Windows pipes.rs or markdown; other agents own those changes.
