---
name: "Vault memory is the primary project store"
description: "Vault Memory/ is the primary project-memory store (mirrored from Claude-side dir 2026-07-04); write new memories to both stores and keep indexes in agreement"
type: "workflow"
scope: "project"
origin: "neoism-agent"
created: "2026-07-04"
updated: "2026-07-04"
---

The Neoism project vault memory (`~/Neoism/Vaults/Neoism/Memory/`) is the primary store for project memories: user-visible in the Alt+N notes sidebar, part of the note graph, and synced across machines by Loro. On 2026-07-04 all 48 topic files from the Claude-side agent memory directory (`~/.claude/projects/-home-parkersettle-projects-neoism/memory/`) were mirrored here and the index rebuilt.

**Why:** The Claude-side store gets its MEMORY.md auto-injected into agent context every session while this vault was pull-only, so agents defaulted to the store they could see. As of 2026-07-04 the agent-server injects this vault's MEMORY.md into the system prompt at session start (`system_memory_indexes` in mcp_memory.rs, wired in session_context.rs) — uncommitted, needs agent-server rebuild to take effect.

**How to apply:** Write new project memories to BOTH stores and keep the two MEMORY.md indexes in agreement. User-type memories (personal/preference/workflow about the person) go to `Default/Personal/Memory/`. Recall is tokenized any-word OR matching as of 2026-07-04 (also uncommitted); multi-word queries now work.
