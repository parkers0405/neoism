---
name: bug-codex-limit-drift-compaction
description: "gpt-5.6 codex-vs-API context split bricked sessions — auto-compact never fired, manual compact 500'd; fixed with auth-aware limit clamp"
metadata: 
  node_type: memory
  type: project
  originSessionId: 61dc832e-208d-41c5-a0b4-dbc86d0b1599
---

2026-07-15: gpt-5.6-sol session (openai OAuth) hit "Your input exceeds the context window of this model" at ~372k prompt tokens; auto-compaction never fired; manual /compact returned HTTP 500 "no usable summary was produced"; UI pill showed 35%.

**Root cause:** OpenAI enforces per-surface windows for the same model id — gpt-5.6-sol is 1.05M context/922k input via platform API but ~372k catalog cap (353.4k effective) on the codex/ChatGPT-OAuth backend, since cut further to ~258k (openai/codex#31860, #32806; same 400k-vs-1M split existed for gpt-5.5). models.dev advertises the API numbers under BOTH `openai` and `github-copilot`, so `apply_codex_openai_effective_metadata`'s copilot-catalog lookup returned 1.05M and the intended codex clamp (CODEX_OPENAI_* = 400k/272k/128k) was a no-op. Auto-compact threshold became 922k−20k=902k (never reached at 372k); manual compaction budget ¾×902k=676k meant the ~370k history replay was never trimmed → the summarize request itself got rejected → zero deltas → None → 500. UI pill reads limit.context via effective_provider_catalog → same hole (372k/1.05M = 35% shown, ~real 93%).

**Fix (provider_catalog.rs):** (1) copilot-catalog entry may only LOWER the codex ceiling, never raise it (per-field min vs constants); (2) whole clamp now gated on `openai_codex_oauth(auth_store)` — mirrors `OpenAiRuntime::stream` dispatch where stored OAuth always wins; API-key path keeps full 1.05M/922k limits AND real per-token costs (old clamp wrongly zeroed costs for API-key users). `generation_metadata`/`effective_provider_catalog`/`usable_provider_catalog` take a `codex_oauth: bool`. Stuck sessions self-heal after rebuild: next prompt trips before-step compaction (usable 252k), budget 189k trims the replay.

**Why:** limit metadata is load-bearing for compaction; the correct limit depends on which auth path the request rides, not just the model id.

**How to apply:** codex-side caps are volatile — if codex effective drops below 252k, sessions brick again; recheck constants against openai/codex issues. Follow-ups not done: compact-on-overflow-error fallback (self-heal even when metadata lies); OAuth Responses 400s drop the response body ("returned 400: … 400 Bad Request") — include body. Diag: agent DB is now Turso at ~/.local/state/neoism/agent.turso.db (sqlite3-readable; agent.sqlite3 stale since 2026-07-10). Pre-existing broken at HEAD: `tests::compacted_summary_trims_messages_already_covered_by_summary` and example `completion_probe`. Related: [[bug-compaction-loop-and-fallback]].
