---
name: "Compaction estimator vs provider usage"
description: "Premature auto-compaction at UI 53% caused by whole-request estimate overriding authoritative provider usage; fixed with OpenCode v2 usage-first trigger."
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-09-19"
updated: "2026-09-19"
---

2026-09-19: Codex OAuth gpt-5.6-sol auto-compacted while UI showed 53% (211,038/400k). OAuth effective trigger is 252k because 65% of 400k=260k is capped by 272k input minus 20k reserve. The remaining early gap came from `run_assistant_step` comparing a coarse whole-request chars/4 estimate (including tools/system/history) against 252k while UI showed authoritative provider usage from the latest completed request. This diverged from OpenCode v2 `overflow.ts`, which uses assistant provider tokens (`total` or input+output+cache) for overflow decisions. Fixed `session_prompt.rs`: `last_known_token_total` scans backward, stopping at compaction boundaries; `compaction_usage_tokens` prefers provider-reported usage and only falls back to whole-request estimation when no reported usage exists, preserving giant first-request protection and existing bounded overflow recovery. Added regression using exact screenshot usage and an intentionally >252k estimate. User config and process env had no overrides. Verification: LSP diagnostics clean and git diff check clean. Targeted cargo test blocked by unrelated current worktree compile errors in v2_routes.rs (missing EventPayload) and workspace_runtime.rs TenantRuntimeKey test mismatches; full fmt check also blocked by unrelated unformatted files/current same-file tenant edits. Do not release-build.
