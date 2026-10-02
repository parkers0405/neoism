---
name: "Lua plugin status overlap and 60 FPS regression"
description: "Plugin state publication serialized full desktop state every frame; status contributions shared native pill origins"
type: "perf"
scope: "project"
origin: "user debug validation"
created: "2026-10-06"
updated: "2026-10-06"
---

---
name: "Lua plugin status overlap and 60 FPS regression"
description: "Plugin state publication serialized full desktop state every frame; status contributions shared native pill origins"
type: "perf"
scope: "project"
origin: "user debug validation"
created: "2026-10-06"
updated: "2026-10-06"
---

# Lua plugin status overlap and FPS regression

Debug showcase exposed two defects once any Lua plugin loaded.

`Application::pump_lua` called `publish_lua_state` on every daemon/frame pump whenever a runtime or plugin existed. Deduplication happened only after building full JSON snapshots for buffers, workspaces, tabs, file tree, notes, agent sessions, terminal sessions, Git, plugin status, and the complete config. This dropped a steady 144 FPS debug session toward 60 FPS. Fix: gate full state publication to 100 ms, publish immediately for initial state, and force a fresh publication before plugin key/command callbacks. Plugin presence no longer performs full serialization every frame.

Declarative `status.left` and `status.right` contributions received the entire status strip, so they painted from the same origins as native mode/CWD and right pill clusters. Fix: `StatusLine` records the measured native left-cluster end and right-cluster start each render; desktop and shared Chrome give custom UI only the padded free interval. Custom UI skips zero-area slots and clamps contribution width to the slot, preventing backgrounds/hitboxes from escaping.

The `ACTION RAIL` demo status contribution was removed from `~/.config/neoism/plugins/dev.neoism.showcase.chrome/init.lua`; it was only a showcase label, not the actual rail.

Verification: `cargo check -p neoism-ui -p neoism-terminal-wasm -p neoism` and targeted `git diff --check` pass. Runtime FPS needs user validation after restarting their debug build.
