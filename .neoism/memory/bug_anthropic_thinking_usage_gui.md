---
name: "Anthropic GUI thinking-token usage"
description: "Anthropic adapter forced thinking usage to zero; optional thinking_tokens breakdown and initial thinking text now forwarded to existing GUI path"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-12"
updated: "2026-10-12"
---

## Root cause and fix

`neoism-agent/crates/neoism-agent-builtins/src/provider_anthropic.rs` emitted `reasoning_tokens: 0` in both FinishStep and Finish, despite streaming reasoning content through ReasoningDelta. Anthropic reports the billed thinking subset as `usage.output_tokens_details.thinking_tokens` on final message_delta (confirmed against Anthropic extended-thinking documentation). Parser now deserializes the optional breakdown, retains its cumulative value across partial usage events, and forwards it in both finish events. Inclusive output_tokens stays unchanged: server `provider_stream_message.rs` subtracts reasoning from output and GUI already reads tokens.reasoning.

The adapter also discarded nonempty thinking content at content_block_start; it now emits ReasoningStart/ReasoningDelta for that initial text before subsequent deltas. No GUI-specific provider filtering or GUI changes were needed. Older/proxy responses without the optional breakdown retain zero; do not invent billed counts from visible thinking summaries or configured budgets.

Regression tests cover thinking deltas/start/end, initial thinking payload, both finish events, hidden thinking with nonzero billed usage, zero/missing/empty breakdowns, and partial usage preserving thinking count. Focused `cargo test -p neoism-agent-builtins anthropic_` passed 7 tests. Existing parallel-tool media grouping changes in this same file predated this fix and were preserved.
