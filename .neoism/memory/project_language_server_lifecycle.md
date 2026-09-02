---
name: "Language server lifecycle architecture"
description: "Source-backed comparison and target architecture for Neoism language-server lifecycle"
type: "project"
scope: "project"
origin: "project analysis of Neoism, local OpenCode, and Zed source"
created: "2026-05-08"
updated: "2026-05-08"
---

Compared Neoism's persistent LSP service with local OpenCode checkout `6e47ae769ed39461b8bce8249a6bf5f2109252ab` and Zed main `4a3e0af532e4ad89baf634f4b94938b98beaa292` on 2026-05-08.

Durable conclusion: keep Neoism's shared persistent LSP service for editor + agent, but make the workspace-host daemon the sole process owner. Combine OpenCode's compact registry and single-flight startup with Zed's LocalLspStore/RemoteLspStore host-proxy split, explicit Starting/Running lifecycle, status events, intentional stop/restart, stderr-backed failures, and bounded graceful shutdown + process-group kill.

Neoism gaps found: `clients`/`broken`/`spawning` fragmented across locks; concurrent startup gets an error instead of sharing the in-flight result; sticky failures without retry/restart; no proactive process-exit state transition; shutdown writes shutdown/exit but does not bounded-wait; status only Running/Broken/Unknown; no public stop/restart; remote LSP ownership not explicit.

Full source-backed study and incremental implementation order: `docs/architecture/language-server-lifecycle-study.md`.
