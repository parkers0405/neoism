---
name: "OpenCode v2 parity work"
description: "OpenCode v2 parity: lazy MCP gateway, prompt cache key, and verified differences"
type: "project"
scope: "project"
origin: "session"
created: "2026-08-10"
updated: "2026-08-10"
---

Compared checked-in opencode/packages/core and packages/codemode, not legacy assumptions. OpenCode v2 read defaults equal Neoism: 2,000 lines and 51,200 bytes. V2's real context advantages are durable bounded tool settlements, compaction-boundary history projection, stable context epoch baseline, and session-derived OpenAI promptCacheKey. V2 mutations under packages/core do not run formatter/LSP; legacy opencode does. Neoism intentionally retains edit formatter/LSP enrichment.

Implemented initial parity in neoism-agent-server:
- provider_tools_for_agent hides all individual mcp__* definitions and substitutes one dynamic `execute` MCP gateway when visible MCP tools exist; API/UI available_tools_for_directory still exposes complete MCP list.
- Gateway description has namespace counts and deterministic full signatures under a 2,000-character catalog budget.
- execute supports native action=search and action=call. Search indexes canonical path, description, and schema; supports namespace, limit/offset pagination, plural normalization, exact path, deterministic ranking. Call re-fetches live MCP availability, permission-filters, accepts canonical or runtime ID, and delegates through existing invocation/permission path.
- OpenAI Responses requests now set prompt_cache_key from ProviderGenerationRequest.session_id; build_provider_generation_request populates session_id from ChatHookContext.
- Tests in agent_tool_registry cover budget, namespace, ranking, schema match, paging, and sanitized paths.

Verification: 4 focused gateway tests pass; git diff --check passes. cargo check initially passed after gateway, but final full check became blocked by concurrent unrelated edits in tool_support/file.rs missing required_string_either/collect_grep_matches/collect_glob_matches imports. Do not repair/revert unrelated work without checking current owner.
