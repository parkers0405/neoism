---
name: "Lua startup poison + slow debug fixed"
description: "Fixed false Lua lock poison and no-init debug runtime regression"
type: "bug"
scope: "project"
origin: "user debug screenshot"
created: "2026-10-05"
updated: "2026-10-05"
---

# Lua startup poison and debug slowdown

Fixed 2026-10-05.

Symptom: debug startup warned `init.lua: Lua plugin state lock poisoned` even when `~/.config/neoism/init.lua` did not exist, then debug UI was extremely slow.

Root cause 1: `LuaRuntime::load` cloned `Arc<Mutex<BuildState>>` into Lua registration closures and then called `Arc::try_unwrap`. The callbacks intentionally remain alive in the Lua VM, so uniqueness could never succeed. The ownership failure was incorrectly mapped to `Poisoned`. Fix: lock the state and `mem::take` its completed build state; added a regression test that loads an init file registering a style.

Root cause 2: desktop unconditionally created a Lua VM and `pump_lua` rebuilt/serialized all buffer/workspace/tree/notes/agent/terminal/Git/config JSON on nearly every event, even with no active runtime. It also republished plugin snapshots and requested redraws after every publish even when panel content was unchanged. Fixes: do not construct Lua at startup or reload when `init.lua` is absent; early-return from `pump_lua` when no runtime exists; only propagate a post-event snapshot when events occurred and callbacks actually changed the snapshot; deleting init.lua transactionally clears runtime, host state, published state, and renderer snapshot.

`config.json` remains canonical and fully valid without any Lua files. `init.lua` is optional and overlays the typed JSON config in memory. Correct docs example for opacity is `neoism.setup({ ui = { window = { opacity = 0.96 } } })`.

Verification: `cargo check -p neoism-lua --features runtime -p neoism` passed; regression test `loaded_module_callbacks_do_not_prevent_snapshot_finalization` passed; `git diff --check` passed. Vendored Lua still adds a one-time cold debug compile cost, but no-init runtime now avoids VM initialization and all Lua state publication overhead.
