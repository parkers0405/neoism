---
name: std::sync::Mutex / parking_lot Mutex are not re-entrant — never call helpers that re-lock from inside a held lock
description: A helper that calls .lock() on the same Mutex while a guard from that Mutex is alive freezes the main thread. Take a snapshot first, or pass the locked guard in.
type: feedback
originSessionId: 97126cba-f109-488f-9a2f-2b00efd4a19c
---
`std::sync::Mutex` AND `parking_lot::Mutex` (which `FairMutex` wraps)
do NOT support re-entrant locking. Calling `self.data.lock()` while a
`MutexGuard` from the same Mutex is alive on the same thread freezes
the thread on `futex_do_wait`. The freeze-watchdog
(`frontends/rioterm/src/freeze_watchdog.rs`) catches this as a
`STALL global_span_in_progress` plus `STALL redraw_not_delivered`
against the affected window.

This bit twice now:
- Git diff panel's `compute_max_body_scroll` / `scroll_card_into_view`
  rewrite — both held a `MutexGuard<PanelData>` while looping and
  called `card_height(&FileChange)` which tried to take the same lock
  for its `expanded` lookup.
- `Screen::update_highlighted_hints` (`screen/mod.rs:9370`) — held
  `terminal.try_lock_unfair()` then called
  `self.terminal_body_mouse_position(display_offset)` which calls
  `terminal_block_source_row_at_visual_row` (`screen/mod.rs:2775`)
  which does `current.terminal.lock()` — the same Mutex re-entered.
  Caught live in a gdb backtrace; main thread's full chain was:
  `application.rs:2058 window_event` → `update_highlighted_hints` →
  `terminal_body_mouse_position` →
  `terminal_block_source_row_at_visual_row` → `terminal.lock()` →
  `parking_lot::raw_mutex::RawMutex::lock_slow` → `park` →
  `futex_wait`. Other threads (PTY readers, tokio nvim runtime) all
  on `do_epoll_wait`, idle — confirms it's NOT cross-thread, it's
  same-thread re-entrancy.

**How to apply:**
- Audit every `&self` helper that calls `self.<field>.lock()`. If any
  caller already holds a guard on that same `Mutex`, you have a
  deadlock.
- Refactor the helper into two flavours:
  - `helper(&self)` — takes the lock itself, for callers without a guard.
  - `helper_for(&self, data: &<Lockee>, …)` — operates on a borrowed
    snapshot, for callers that already hold the guard.
  Reuse the snapshot variant inside a loop so the lock is held once
  for the whole pass.
- When in doubt, `let snapshot = { let g = m.lock()…; g.clone() };`
  releases the lock before any other call. Cloning is usually cheaper
  than chasing a freeze.
- Background work that touches the same Mutex must spawn *after*
  dropping the guard, never inside the guarded block.

**Diagnosis playbook (when frozen, before killing the process):**
1. `pgrep -af neoism` to get the pid.
2. `sudo bash /tmp/neoism-freeze-bt.sh` (or
   `sudo gdb -p <pid> -batch -ex "set pagination off" -ex "thread apply all bt 30" -ex "quit" > /tmp/bt.txt`).
3. Find the main thread (look for `"neoism"` LWP near the top of the
   pid). The frame chain shows exactly which function holds the outer
   lock and which inner call re-enters.
4. PTY readers / nvim runtime in `do_epoll_wait` confirms it's NOT
   cross-thread; the freeze is the main thread's own re-entrant lock.
