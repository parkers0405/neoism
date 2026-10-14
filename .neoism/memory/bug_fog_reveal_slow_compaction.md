---
name: "Fog-Reveal slow compaction investigation"
description: "Fog-Reveal chat: compaction succeeded in 265s; 288k history/high invisible reasoning; dotted GPT-6 model bypasses OAuth limit predicate"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-09"
updated: "2026-10-09"
---

Read-only investigation of session `ses_eeb6cf51effez0s2JVnVBuClA5`, title `Fog-Reveal Streaming Animation for Rust GUI`, October 9 2026. Model `openai/gpt-6.1-sol`, variant high; default selected OpenAI connection OAuth (no session override). Manual compaction succeeded: 19:21:11.657 to 19:25:36.649 UTC, 264.992s; HTTP /v2/sessions/.../compact returned 204 with 265019ms latency in workspace-service.log:2897204. Model summary 30,174 chars and throughMessageID persisted. Prior normal step 19:12:27.231 to abort 19:20:43.183 (495.952s), 18 reasoning parts with empty visible text arriving every ~21.6-32.6s; actual provider activity rather than daemon freeze. Earlier completed step 469s with 8,478 reasoning tokens and 252 output tokens. Latest normal provider input+cache-read 288,203 tokens, only 3,200 cache-read; 269 messages/1,225 parts before summary completion, no prior completed compaction. Stored bytes are NOT token counts.

Confirmed current-source gap: builtins provider_catalog.rs:1009-1015 is_codex model predicate matches gpt-6 and gpt-6-* but misses dotted gpt-6.1-sol, so OAuth metadata clamp is bypassed. Cached catalog 1.05M context/922k input/128k output and configured 75% threshold yields proactive trigger 787,500; compaction budget ~676,500. Consistent with huge un-compacted history but exact installed binary logic not established; actual route handled >272k tokens so DO NOT blindly assert legacy 272k constant as this model/account hard limit. Need authoritative route-specific metadata and dotted-model coverage if fixing.

session_context.rs:550-565/622-630 timeout 240s is inter-event, not total; reasoning events can keep compaction alive past 4m without visible text (event handling 567-584). Compaction deltas publish_live (937-961), not DB rewrites per delta. Message endpoints 13-85ms/runtime mostly 97-238ms during slowdown argue against DB-wide freeze; no current desktop frame trace, cannot exclude UI jank. Diagnose dominant prolonged provider reasoning over giant history plus weak visible compaction progress. No code/config/chat mutations made; request was diagnosis. DB ~/.local/state/neoism/agent.turso.db queried immutable=1 due mode=ro lock; immutable lacks WAL transaction consistency, independent daemon HTTP log confirms outcome. Recommendations: route-aware limit resolution, elapsed/last-event compaction progress, phase timing telemetry, mocked reasoning-only SSE timeout tests.
