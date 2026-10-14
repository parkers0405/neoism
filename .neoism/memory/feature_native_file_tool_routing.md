---
name: "Native file tools first in harness"
description: "OpenCode-style native file tool routing added to Bash/background descriptions, shared context, and Build/Plan/Explore prompts"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-12"
updated: "2026-10-12"
---

## Native file tools first

Neoism models were choosing shell file operations because bash's description only said 'Run shell commands', shared workspace context lacked a clear routing rule, and Explore explicitly allowed read-only shell file operations/listings. General/custom agents rely on workspace context rather than Build's engineering prompt. Upstream OpenCode literal 2.0 reinforces native tools in Bash's description and model prompts; current dev also has shared tool guidance. This is instruction-based routing, not hard shell-command rejection.

Implemented in `neoism-agent/crates/neoism-agent-server/src/tool_registry.rs`: Unix Bash and Windows PowerShell descriptions explicitly reserve shell for Git/build/test/package/Docker terminal operations. Discovery -> glob; content search -> grep; file/directory reads -> read; mutations -> selected apply_patch or edit/write. Native tool descriptions reciprocally say to use them instead of shell equivalents, while preserving existing search/pagination guidance. background_task has the same routing rule, preventing an instruction loophole via background shell commands.

Implemented in `neoism-agent/crates/neoism-agent-builtins/src/plugin/system_prompt/mod.rs`: shared 'Native file tools first' workspace context applies to primary agents and subagents, including General/custom agents, preserves editingTool-specific mutation guidance. Shell fallback is allowed only when the required native tool is unavailable or cannot support the operation; familiarity or pipeline convenience is not an exception.

Implemented in `neoism-agent/crates/neoism-agent-builtins/src/plugin/agents/native.rs`: Build/Plan engineering rule strengthened; Explore uses lowercase tool IDs and no longer allows shell directory listings/searches merely because they're read-only. Explore retains its no-state-change rule and may use read-only terminal operations such as git status/log.

Regression tests cover shared context for both mutation tool selections and custom instruction fragments, Build/Plan/Explore prompts, shell/background/native advertised descriptions (Windows-specific assertions conditional on Windows). Builtins suite passed 128 tests. No runtime command parser/blocklist was introduced and permissions remain unchanged.
