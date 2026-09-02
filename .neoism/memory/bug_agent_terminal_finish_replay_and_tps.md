---
name: "Agent terminal Finish replay and TPS"
description: "Agent answers no longer replay after Finish; OpenCode V2-compatible output TPS appears in completed response footer"
type: "bug"
scope: "project"
origin: "session 2026-08-26"
created: "2026-08-26"
updated: "2026-08-26"
---

## Root cause

Agent V2 processed a terminal `ProviderStreamEvent::Finish` but continued waiting for the provider transport to reach EOF. Some SSE transports remain open after the terminal event. The idle timeout then classified the already-complete text-only step as retryable, reset its live text, and visibly streamed the same answer a second time.

## Fix

`provider_stream_processor::run_provider_stream_step` now exits its receive loop immediately after successfully processing `Finish`. `FinishStep` remains nonterminal. Regression: `finish_event_terminates_without_waiting_for_transport_eof`.

## OpenCode V2 TPS parity

At upstream `anomalyco/opencode` branch `v2`, commit `33909f48`, TPS is `sum(assistant tokens.output) / sum(assistant (time.streamed - time.created))` over all assistant model steps since the nearest user/synthetic turn. It omits TPS if any step lacks a stream boundary and excludes input, cache, reasoning, and tool-only completion gaps.

Neoism added optional `CompletedTime.streamed`, stamps it at provider terminal completion, computes the same multi-step formula in shared `api_mapping.rs`, and displays one decimal in the existing completed answer footer: `Build · Model · duration · 32.0 tok/s`.

## Landed

Commit `edcbeee6b` on `neoism_agent_v2`, pushed 2026-08-26. Focused server/UI regressions and `cargo check -p neoism-agent-server -p neoism-ui` passed.
