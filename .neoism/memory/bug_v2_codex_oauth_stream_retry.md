---
name: "V2 Codex OAuth stream retry — FIXED"
description: "V2 Codex OAuth stream retries fixed by preserving structured retry metadata across ProviderService and rejecting premature EOF"
type: "bug"
scope: "project"
origin: "session"
created: "2026-08-28"
updated: "2026-08-28"
---

## Root cause

Neoism agent v2 routes generation through `ProviderService`. `neoism-agent-builtins/src/provider_service.rs` converted builtin `anyhow::Error` values to `PluginRuntimeError` with only `error.to_string()`. The server then stringified that again in `session_prompt.rs`. This erased `ProviderError.retryable`, `retry_after_ms`, and typed `reqwest::Error` transport classification, so Codex OAuth stream failures became fatal unless the outer text happened to match retry phrases.

A second bug in `provider_stream_processor.rs` treated transport EOF before `ProviderStreamEvent::Finish` as successful completion, committing partial output.

## Fix

`PluginRuntimeError` now carries `retryable: Option<bool>` and `retry_after_ms`. `ProviderPlatform` classifies the original provider/reqwest error before crossing the plugin boundary and retains its full causal message. The server wraps `PluginRuntimeError` as a typed anyhow error rather than stringifying it, and retry classification/backoff consumes the metadata. `Option<bool>` preserves explicit terminal provider decisions while allowing message fallback for unclassified third-party plugin errors.

EOF before `Finish` now returns a retryable unfinalized step error when no tool call was executed. After any tool call, it finalizes terminally to avoid replaying side effects.

## Verification

`cargo check -p neoism-agent-builtins -p neoism-agent-server` passes. Builtin metadata tests, all `session_retry` tests, and EOF/Finish guard tests pass. Workspace-wide rustfmt remains pre-existingly noisy; `git diff --check` passes.
