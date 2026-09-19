---
name: "Optional TypeSafe computer MCP mode"
description: "Dependent exact-action risk request fixed Jev Choice independence bug; actionable DOM candidates hardened."
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-09-18"
updated: "2026-09-18"
---

Implemented dependent Jev decision pipeline in `computer_use/typesafe.rs`: selection request now batches only Choice + done, validates unchanged confidence >=.85 and selected probability >=.75 gates, resolves the exact candidate, then issues a separate structured-Noul risk request scoped to exact action/ref/value or URL plus current URL and selected target context. Risk >=.1 remains `needs_confirmation`; malformed/missing risk fails closed. Ambiguous/no-match/possibly-done selection skips risk. browser_step uses a 16s aggregate decision budget; browser_goal's existing total deadline covers selection+risk. Both recheck cancellation/revocation/enablement around risk and before dispatch; preview never dispatches. Results expose `selectionMs`, `riskMs`, `decisionMs`, and stage.

`browser_observe.js` now emits `actions`, `actionable`, link `href`, and bounded nearby context; it filters headings/presentation/containers and other non-actionable role/tabindex nodes. TypeSafe candidate generation requires the explicit compatible action list, preventing click candidates for headings while leaving manual browser_act mechanics unchanged.

Bundled TypeSafe docs now explain the dependent second risk request, compact candidate semantics, stage timings, and that needs_confirmation remains a hard stop without broad confirmation UI.

Verification: 12 TypeSafe tests pass; 12 browser unit tests pass (1 live Firefox ignored by fixture design); `cargo check -p neoism-agent-server` passes with pre-existing unrelated warnings; 2 neoism-product-docs tests pass; browser_observe.js passes `node --check`; edited diff passes `git diff --check`. Added regressions for exact selected action context/no hypothetical candidate, risk-call elision on ambiguity/done, high-risk dispatch stop, malformed distributions/risk, all mutation-capable operation risk requests including navigate, and no heading click candidates.
