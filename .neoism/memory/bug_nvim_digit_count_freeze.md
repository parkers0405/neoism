---
name: bug-nvim-digit-count-freeze
description: Bare digit in nvim pane freezes input — nvim defers non-fast RPC while count pending; both RPC drivers let a deferred command block the input lane
metadata: 
  node_type: memory
  type: project
  originSessionId: 8c8ab78f-b2bc-4a51-9d08-a3e1f6c67725
---

Pressing a bare digit (1-9) in the embedded nvim pane "freezes" the editor. NOT a key-translation bug (digits pass through correctly). Root cause: a pending count leaves nvim in an unsafe state where it defers all **non-fast** RPC (`nvim_command`, `nvim_exec_lua`) until the count resolves; only `FUNC_API_FAST` calls (`nvim_input`) still answer. Proven empirically against nvim 0.12.3 + the rio lua runtime.

Two freeze paths sharing the root cause:
- **Path A (desktop, 30s freeze)**: single sequential RPC pump (`neoism-backend/src/performer/nvim.rs:1732`) — inputs and commands share one lane. A queued Command (30s timeout) blocks every later keystroke including the Esc that would clear the count. Worse: Esc itself sends `nohlsearch` **command before** the Esc input (`editor_scroll.rs:529`, classify at `scroll_model.rs:1481`). Also triggered with no second key by ACP agent `:checktime` (`screen/bridges/acp.rs:360`).
- **Path B (daemon/web, unbounded freeze)**: 2s diagnostics tick awaited inline in the per-connection websocket `select!` loop (`neoism-workspace-daemon/src/server.rs:2929-2943`); `snapshot_diagnostics`/`snapshot_lsp_states` (`daemon nvim.rs:2394-2450`) call `exec_lua` with NO timeout → permanent deadlock (nvim waits for input, daemon waits for exec_lua, input never read off socket).

Why digits feel special: `showcmd=false, cmdheight=0` (`nvim_runtime/lua/rio/options.lua:28-34`) → no count indicator, so users pause on the pending state; pending operators (d/y/g) can wedge too.

FIXED (2026-07-03): desktop pump got a separate sequential slow lane (`slow_tx` in run_nvim_runtime) for Command/Resize; ClearSearchHighlightThenSend reordered Esc-first; daemon snapshots gate on `nvim_get_mode().blocking` (FUNC_API_FAST — empirically true exactly during pending count, false in op-pending), clone the client out of BOTH mutexes before RPC, use `nvim_rpc_timeout`, return Option so skipped polls don't push spurious DiagnosticsCleared; server.rs runs the poll as a detached single-flight task (`DiagnosticsSubscriptions::fetch`/`apply` split). Also enabled `showcmd` + new `msg_showcmd` channel (parse_showcmd → drain_showcmd → Context.editor_pending_keys → StatusInfo.pending_keys): mode pill shows "NORMAL · 2d" so pending counts are visible. Fix principle: **never let a non-fast RPC block the input lane**. Related: [[bug-nvim-freeze]].
