---
name: "Sidebar opacity debug test override"
description: "Sidebar opacity test now commented out in personal init.lua; defaults restored, remove two Lua comment markers to re-enable"
type: "preference"
scope: "project"
origin: "neoism-agent"
created: "2026-10-08"
updated: "2026-10-08"
---

LATEST USER REQUEST: turn sidebar opacity OFF but retain easy toggle. Commented entire personal opacity block in ~/.config/neoism/init.lua using Lua --[[ ... ]] longcomment; header says remove marker lines to re-enable. Block retained sidebar_opacity0 and fourselectorloop agent.sidebar/file-tree/notes-tree/git-sidebar, explicittransparentonlyat0. LuaCLI verified executingcurrentfile makesZERO ui.style calls. Watcherhotreload restoresdefaultsidebarappearance onupdateddebugbuild. No wallpaper/theme/tab/topchrome/pack changes. Previoustest wasuserauthorizedfulltransparency; NOWDISABLED. Don't re-enable withoutuserasking. To enable removeonlytwo markerlines; preserve anyfutureothercustomizations. Config::apply_json_patch({}) no-op bug fixed see bug_style_only_lua_empty_config_patch.
