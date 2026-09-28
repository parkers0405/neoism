---
name: "Skill generation lease refresh"
description: "Skill generation re-lease on retired snapshot fixed and live-verified; release published"
type: "bug"
scope: "project"
origin: "live computer test and release workflow"
created: "2026-09-23"
updated: "2026-09-24"
---

2026-09-23 fixed intermittent built-in Skill 'tool plugin generation was not provided': plugin call held pinned PluginGenerationLease but BuiltinTool with_generation re-leased numeric generation from runtime; when generation retired, reacquire failed and skill context lacked snapshot. tool_runtime now scopes runtime.execute with existing lease; ToolContext::with_generation prefers scoped lease only if same generation, root, and owning tenant runtime (WorkspaceRuntime::owns_generation checks published or retired weak ptr), else falls back to lease_generation. Added pinned_session_tools_survive_plugin_generation_refresh regression invoking skill and edit after refresh; passed. Live computer-use test in separate rebuilt debug window: Skill completed for neoism-yolo-release and model reported heading, no release launched in that test. YOLO v0.7.111 subsequently tagged 12dcd5df48c10ad40a5370e0fb5b4564318d560a, run 35925744644 SUCCESS (Linux/macOS/Windows + publish). Public release https://github.com/parkers0405/neoism/releases/tag/v0.7.111, undrafted, 8 assets including Windows MSI (not ZIP). Local .neoism/memory notes excluded from public commit.
