---
name: "OpenCode v2 parity rule"
description: "Implementation rule: source-verify overlapping agent behavior against local OpenCode v2 and match it unless Neoism requires an explicit divergence."
type: "feedback"
scope: "project"
origin: "User explicitly requested this after OAuth/context-growth fixes."
created: "2026-08-12"
updated: "2026-08-12"
---

## Rule

For Neoism agent runtime behavior that overlaps OpenCode, inspect the local OpenCode v2 implementation before designing or changing behavior. Use its active source paths and verify the exact semantics, constants, lifecycle timing, persistence behavior, and provider replay behavior. Prefer direct parity over an approximate solution unless Neoism has a concrete architectural constraint; document any intentional divergence.

This especially applies to context management, compaction, pruning, provider transforms, OAuth/Codex routing, usage accounting, tool-output truncation, queueing, and session replay. Do not rely only on comments, memory, or broad assumptions: trace the active local OpenCode code path and tests.
