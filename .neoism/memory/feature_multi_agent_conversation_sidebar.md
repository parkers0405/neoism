---
name: "Multi-agent conversation sidebar"
description: "Native Rust four-provider chat and ACP history integration"
type: "feature"
scope: "project"
origin: "2026-09-24 implementation and verification"
created: "2026-09-24"
updated: "2026-09-24"
---

Workspace-level Conversations sidebar: native desktop Rust GUI and shared web Chrome, Alt+C/topbar. Files/Notes visibility restored on close. Two-line date-grouped source-icon/title/time rows; search, paging, pin/rename/delete Neoism sessions. Right detail panel chat-only, active/wide/unhidden; Back and duplicate session list removed. Four ConversationSource values: Neoism Agent, Claude Code, Codex, OpenCode.

2026-09-24 expansion: New Chat chooser on native Rust GUI and web creates first-class Neoism root sessions for all four. `POST /v2/sessions` optional `externalProvider` opencode|claude|codex, omitted for Neoism; web daemon `CreateThread.external_provider`. Root immutable `extra.externalAgent` routes queued text prompts through workspace-owned ACP server, preserving Neoism timeline/SSE/events, permissions, abort, statuses, ACP tool parts, inferred child agent sessions, and structured ACP plans persisted as Neoism todos. OpenCode uses `opencode acp`; Codex pinned `@agentclientprotocol/codex-acp@1.13.1`; Claude pinned `@agentclientprotocol/claude-agent-acp@0.81.1`. Resume requires advertised load/resume, never silently creates a new provider session. Host-owned provider credentials; hosted multi-tenant root ACP creation disabled. Interactive GUI login not implemented; sign in with provider CLI if required.

Native sidebar asynchronously fetches capability-gated GET `/v2/sessions/external/catalog?provider=...&directory=...` for OpenCode/Codex/Claude, merges/dedupes imported roots, shows provider-specific errors. On click POST `/v2/sessions/external/import` if importSupported; load-replays real user/assistant text into a hidden importing root, makes visible after success, dedupes repeated imports. No fake empty chats; import is text_only and a persistent native timeline note discloses omitted historical tool cards/non-text. Exact cwd, local operator and source-host checks prevent cross-workspace history leak; foreign host imported chats cannot resume. New turns stream full ACP subset. Desktop Agent tab icon/title/composer/detail reflect provider; Neoism-only model/mode/attachment/slash controls hidden for external text-only chats. Web thread summary carries external_provider. Native GUI is user's priority; web parity for provider-native catalog/import not built.

Critical fix: `external_agent/helpers.rs::update_external_session_metadata` must clone/patch existing externalAgent, not replace it, or first prompt erases sourceHost/sourceKey/historyState/planTodos. Regression test `resumed_import_keeps_source_host_and_plan_metadata`. Daemon ServerOptions hosted_attestation: None compile fix. Checks: 21 `external_agent` tests pass; `cargo check -p neoism -p neoism-terminal-wasm` passes; npm run typecheck and git diff --check pass. ACP initialize-only probes passed for three local adapters; live signed-in provider prompt/GUI run not performed. Historical ACP import text-only by design; full historical tool/subagent transcripts and interactive GUI auth remain gaps. Many unrelated concurrent worktree edits; don't reset them.
