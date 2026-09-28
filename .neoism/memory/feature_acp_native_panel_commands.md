---
name: "ACP native panel and commands"
description: "Native ACP options pending/error and immediate catalog reconciliation"
type: "feature"
scope: "project"
origin: "2026-09-24 follow-up user task"
created: "2026-09-24"
updated: "2026-09-24"
---

2026-09-24 follow-up frontend native ACP: existing New Claude/Codex/OpenCode queues EnsureSession; root success already calls maybe_refresh_external_options; added pending and error state to composer chip row and Slash picker (header-only loading/empty, actionable Retry row, no Neoism command fallback), draft survives root creation and options hydration, source switch rejects stale snapshots. Non-409 GET errors stop automatic retry, explicit chip/picker retry resets and dispatches GET; root creation failure offers same retry via EnsureSession. First options snapshot whose externalSessionId binds signals native screen bridge to refresh Conversations current sessions + provider catalog immediately (even hidden) so shared sourceKey/neoismSessionId reconciliation prunes only matched preview. Tests native external_options::tests (7), identity pruning, provider_ (10), shared chrome (18); cargo check neoism/neoism-ui/wasm and git diff --check pass. No live signed-in provider GUI verification.
