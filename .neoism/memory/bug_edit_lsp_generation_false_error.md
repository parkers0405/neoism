---
name: "Edit succeeds but reports plugin-generation error"
description: "Edit mutation succeeds then tool reports error"
type: "bug"
scope: "project"
origin: "session"
created: "2026-09-23"
updated: "2026-09-23"
---

2026-08-05: Neoism Edit tool sometimes returned `tool plugin generation was not provided` despite modifying disk. Root: BuiltinTool::execute called ToolContext.with_generation before with_session_id, so tenant lookup defaulted to local and guest lease absent; post-write metadata in tool_support/file.rs (write/edit), patch_tool.rs unconditionally used context.lsp_runtime()? after commit. Fixed order (session first, generation next); post-write LSP + snapshot reporting now optional with explicit metadata `lspUnavailable` / `snapshotError`, preserving successful mutation output. Tests: direct Write/Edit/Patch with no generation assert disk + success + lspUnavailable; guest tenant pinned runtime Edit asserts no lspUnavailable, normal lspTouch, content updated. Note tool API may still return a failed result from old running server until rebuilt/restarted. Tool-result error after successful mutation can also arise from other post-hooks, separate follow-up.
