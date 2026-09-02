---
name: "Agent Markdown inline span overlap — FIXED"
description: "Agent Markdown styled spans no longer overlap following plain text; placement uses actual shaped draw advance instead of trusting stale cached measurement."
type: "bug"
scope: "project"
origin: "session"
created: "2026-08-31"
updated: "2026-08-31"
---

Agent Markdown paints inline styles as separate Sugarloaf runs. Blue/bold link text could overlap the following plain text because draw used current shaping while X advancement came from a process-global measurement cache whose key has text/font size/style/font_id/scale but no Sugarloaf instance or font generation. Fix: `draw_text_clipped` now returns Sugarloaf's exact shaped draw advance (and directly measures in occlusion mode), while `draw_markdown_inline_run` advances by max(actual draw advance, conservative monospace measurement). This keeps following spans outside painted glyphs even after font state changes. Files: neoism-frontend/shared/src/panels/agent_pane/view/{draw.rs,markdown.rs}. Verified cargo check -p neoism-ui, focused test, rustfmt check.
