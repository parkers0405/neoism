---
name: bug_nvim_freeze
description: "Year-long recurring nvim pane freeze — RPC pump deadlock diagnosis, evidence pattern, mitigations, capture tooling"
metadata: 
  node_type: memory
  type: project
  originSessionId: 3eff29bd-a895-4c7f-98f6-b44fd5974e1b
---

2026-06-12 live capture of the recurring (year-long) nvim freeze before the
user killed it. Evidence: neoism main thread ALIVE (R, ~10% CPU, event loop
running), embedded nvim (`--embed --headless --clean`) idle in epoll_wait, the
RPC pipe pair intact on both sides, no suspicious futex waiters → a LOGICAL
deadlock, not a lock or pipe stall.

**Mechanism (high confidence):** the nvim runtime command loop
(`neoism-backend/src/performer/nvim.rs`, "Service commands until Shutdown")
awaited every RPC (`input`, `command`, `input_mouse`, `ui_try_resize`)
inline with NO timeout. If one response is lost (reader hiccup) or nvim
blocks in a prompt needing input (`vim.fn.input()`, confirm(), `:!` reading
stdin — ext_messages doesn't cover all of these; LSP runs in the embed), the
pump hangs on that await forever and can never deliver the very input that
would unstick nvim. Both processes sit in epoll — matches the capture
exactly.

**Mitigations landed:** every pump RPC is timeout-bounded (10s input/mouse/
resize, 30s command) + a degraded mode (after first timeout, 250ms bounds
until a call succeeds) so the pump survives and the user's next Esc/Enter
reaches nvim — freezes self-recover. Timeouts log `tracing::error` with the
offending keys/command. Also: the freeze watchdog
(`desktop/src/app/freeze_watchdog.rs`) is OPT-IN as of 2026-07-14
(NEOISM_FREEZE_WATCHDOG=1) — always-on default filled `<config>/log/` with
241MB of per-frame NOTE files (74MB single session), user asked to stop.
Same day: editor-grid diag file gated behind NEOISM_EDITOR_GRID_LOG=1 (its
shadow-editor SELF-HEAL in the context pump still runs regardless);
`freeze_watchdog::init()` now sweeps both log families on every launch
(>3 days, or entirely when the family is disabled).

**Next freeze playbook:** relaunch with NEOISM_FREEZE_WATCHDOG=1, then run
`sudo scripts/freeze-dump.sh` (auto-finds pids, writes /tmp/neoism-freeze-*.txt
with 3 stack samples + thread states + fds), and grab the freeze-watchdog log.
ptrace needs root (yama scope 1). The error-level timeout logs will name the
exact stuck RPC + keys.

Note: embedded nvim relocates RPC off stdio (fd0/1/2 → shared stderr pipe
write-end is NORMAL; the real channel is a separate pipe pair).
