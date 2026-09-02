---
name: "Atomic V4A apply_patch"
description: "V4A apply_patch now preflights atomically, rolls back commit failures, and surfaces context; not Codex-specific"
type: "bug"
scope: "project"
origin: "project source investigation and fix"
created: "2026-07-17"
updated: "2026-07-17"
---

# Atomic V4A apply_patch

## Root cause

`apply_v4a_patch_locked` in `neoism-agent/crates/neoism-agent-server/src/tool_support/patch_tool.rs` executed V4A hunks directly against disk in order. If a later file or context failed, earlier files remained mutated while the tool returned an error naming only the failed file. This was provider-independent; Codex OAuth uses the same strict `patchText` tool schema.

## Fix

V4A operations now resolve against an in-memory virtual filesystem first, preserving ordered same-path operations. No disk writes happen until every hunk validates. Commit-time filesystem failures restore all captured pre-patch `FileState`s through `snapshot::write_state`, and errors report rollback status. Context errors now include the complete underlying context-not-found chain. Added a regression test proving a stale second file leaves the valid first file unchanged.

## Runtime caveat

The Agent server process must restart before its own `apply_patch` MCP tool uses newly edited server code. During the implementing session, subsequent oversized patches still exhibited old partial-apply behavior because the running binary was unchanged.
