---
name: feedback-tasks-vault-parallelism
description: Canonical backlog is the vault TASKS.md; fan one worktree sub-agent per section for big multi-area lists
metadata: 
  node_type: memory
  type: feedback
  originSessionId: a93ebab4-d034-468d-9031-ca746f2b9efd
---

The user's canonical task backlog lives at `/home/parkersettle/Neoism/Vaults/Neoism/TASKS.md` (an Obsidian-style vault, capital `Neoism`). It's organized by `### Section` headers (Neoism Agent / Notes / Markdown / Chrome / Terminal / Web) with `- [ ]` / `- []` checkbox items. As work completes, edit that file to flip items to `- [x]` and append a terse `(root cause + fix + branch)` note inline — the user explicitly wants items marked complete there.

**Why:** It's their persistent source of truth across sessions; the chat is ephemeral.

**How to apply:** On a big multi-section dump, don't serialize it all yourself. The user's preferred model (stated directly): "finish one section at a time, have sub-agents work on this — you handle one section, another sub-agent a full section, etc." So: take the section with the most loaded context yourself, and spawn one `general-purpose` sub-agent **per other section** with `isolation: "worktree"` + `run_in_background: true`. Each commits to its own `worktree-agent-<id>` branch; relay branch + root cause + gaps and check the section off in TASKS.md as each reports. This is one of the rare times to spawn agents — the user asked for it. See [[feedback-keep-building]] and [[feedback-agent-worktree-gotchas]] (agents' cwd teleports to main repo; they must cd to their worktree before git ops).
