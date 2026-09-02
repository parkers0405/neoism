---
name: "Agent timeline reader teleport during streaming — FIXED"
description: "Golden-standard agent timeline anchoring and optimistic prompt identity fixes completed after post-fix audit."
type: "bug"
scope: "project"
origin: "Post-fix audit implementation in Neoism workspace"
created: "2026-08-23"
updated: "2026-08-23"
---

Post-fix audit completed. User-authored idle prompt paths now allocate one outbound MessageId before optimistic insertion and reuse it for the bubble and SendPrompt in desktop/shared (including goal/FX paths). TimelineViewAnchorKey resolution is ambiguity-safe: no length-delta-as-prepend inference; unique durable IDs resolve directly; duplicate durable IDs require signature validation and only accept agreeing start/end ordinals; removed duplicate rows return None; legacy empty IDs use unique signature matching and can reconcile after movement plus empty→durable transition. Pending prompt reconciliation consumes server and local occurrences one-to-one and preserves unresolved optimistic bubbles/IDs. Anchor restoration shifts active wheel targets by the actual resulting scroll delta in desktop/shared. Desktop structural stable-prefix classification rejects duplicate-ID remaps unless exact rows match. Focused tests cover prompt ID reuse, duplicate durable removal, legacy optimistic movement/transition, append vs prepend, identical prompts, wheel target correction, grouped boundary, and duplicate-ID structural snapshots. Verified focused shared/desktop tests, cargo check neoism-ui+neoism, fmt, and diff check.
