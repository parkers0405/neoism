---
name: "Transparent sidebar Lua silently rejected at config patch gate"
description: "Style-only init.lua accepted by Lua but discarded by Config empty-patch roundtrip; fix empty-object no-op gate, debug rebuild/relaunch needed"
type: "bug"
scope: "project"
origin: "live user report + isolated native read-only probes"
created: "2026-10-08"
updated: "2026-10-08"
---

# Style-only personal Lua rejected before snapshot publication

Diagnosed 2026-10-08 against live debug PID 231424. Isolated read-only probes using existing native neoism-lua/backend rlibs loaded ~/.config/neoism/init.lua successfully: agent.sidebar/file-tree/notes-tree/git-sidebar resolved background="transparent", opacity=0.0. Config::try_load succeeded, but apply_json_patch(runtime.snapshot().config_patch) rejected the empty object with `invalid type: map, expected a sequence`. serde_path_to_error localized the first full Config serialization/deserialization mismatch to ui.margin (Margin derives map Serialize but accepts only CSS array Deserialize). A further probe normalizing margin representation exposed ui.navigation.unfocused-split-fill null/string mismatch too; general nonempty config-patch roundtrip remains a separate issue.

Desktop startup main.rs discards the runtime on this gate failure; apply_update_config retains prior config/runtime/snapshot transactionally. Thus sidebar paint never receives the valid personal styles, despite correct native Lua loader, recursive directory watcher (live inotify registration confirmed), personal-over-plugin merge, frame snapshot bracketing, base_color opacity helper, and ring-only translucent frame backing. Existing executable contained those helpers; no stale helper diagnosis was warranted.

Minimal fix in neoism-backend/src/config/mod.rs: treat {} as a genuine no-op alongside null before serializing Config. Added style_only_lua_empty_config_patch_is_a_noop regression ensuring unchanged config and array-root rejection. cargo check -p neoism-backend -p neoism --tests passed (tests compiled, not run). No personal config/style edits or running window restart. Debug binary mtime 2026-10-08 11:28:05 -0500; fix source mtime 11:50:44 -0500. Parent must rebuild debug and relaunch when safe for visual verification; source changes cannot hotreload into PID231424.
