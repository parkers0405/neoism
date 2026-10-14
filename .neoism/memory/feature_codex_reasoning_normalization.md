---
name: "Codex reasoning normalization and compaction visibility"
description: "FIXED + verified: capability-driven Codex reasoning, final SSE summary backfill, visible compaction reasoning desktop/web; OAuth/API context split preserved"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-09"
updated: "2026-10-09"
---

Implemented October 9 2026 after Fog-Reveal diagnosis (see bug_fog_reveal_slow_compaction.md). Previously GPT-6.1 model missed a GPT5/codex readable-summary whitelist. Shared Responses reasoning requests now use active effort or model reasoning capability; preserve explicit summary overrides/null and effort:none. ProviderApiInfo gained optional `reasoning` separately from reasoning_effort compatibility; generation_metadata forwards ModelInfo.capabilities.reasoning. Native API-key and Codex OAuth Responses streams share post-merge responses_body normalization; capability true can request summary:auto with no variant and no invented effort, even reasoningEffort:false. Nonreasoners don't receive default reasoning. OAuth route still always Responses; API-key Responses routing retains shipped GPT5.6-only heuristic (other compatible adapters ChatCompletions). Do not broadly switch old models/protocols without support data.

Responses SSE parser backfills summary_text from .done and final-item snapshots, deduplicates per item/summary index and missing suffixes; opaque encrypted content only replay metadata, never visible text. Divergent final snapshots aren't replacements for already-streamed content.

Compaction session_context.rs carries mutable assistant message through streaming, forwards normal ReasoningStart/Delta/Metadata/End via transient live events, keeps raw summary text separate, persists reasoning at final success/failure, closes open reasoning parts at completion/cancellation. Initial compaction assistant no longer pre-marked completed (necessary for LiveMessages reconnect snapshot). End provider activity segment on cancellation/timeout/error as well as success. No per-token DB rewrites.

Shared api_mapping.rs compaction branch preserves reasoning before one summary/error card. Desktop ingest grouping handles direct message-keyed Compaction as well as part-keyed Assistant and prevents reasoning deltas from appending to summary. Shared state/ingest.rs has same collision guard; live reasoning deltas reorder only their directly identified compaction card, with user-turn boundary protection. Snapshot/reconnect and final summary remain separated from thinking text.

provider_catalog.rs subscription ceilings now apply based on selected native OpenAI OAuth route instead of model-name whitelist; preserve smaller known model limits, retain larger API limits for API-key requests. Provider service already scopes model_metadata auth to UserModel.connection_id; explicit connection overrides remain respected. Existing 400k context/272k input/128k output ceilings are conservative product ceilings, NOT proof of current upstream hard limits. Catalog can lower ceilings.

FINAL verification passed: 23 Responses parser/body tests, 6 OpenAI normalization tests, 12 catalog/auth-limit tests, 48 shared api_mapping tests, 17 shared compaction tests including final ordering/isolation, 15 desktop reasoning tests, 22 server session_context tests including streamed reasoning/reconnect/completion/abort. Final cargo check -p neoism-agent-server --tests and builtins check passed, with unrelated existing warnings. During verification a missing live-message seed in new test fixture was corrected; unrelated concurrent ServerOptions.hosted_attestation compile errors resolved by their owner before successful final rerun. No release build, live paid request, auth/DB mutation, app restart, commit, or unrelated changes reverted.
