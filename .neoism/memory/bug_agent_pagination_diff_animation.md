---
name: "Agent pagination + diff animation"
description: "Initial pagination storm and ApplyPatch per-file animation invariants"
type: "bug"
scope: "project"
origin: "Implementation and regression verification in Neoism workspace"
created: "2026-07-14"
updated: "2026-07-14"
---

# Agent pagination and per-file diff animation

- Initial history pagination must require `timeline_follow_bottom == false`. `render_timeline` evaluates the boundary every frame, so boundary-only gating causes a fresh/short transcript to serially fetch many pages. The follow-bottom gate keeps initial load quiet and naturally arms continuous pagination after deliberate upward scrolling.
- ApplyPatch file expansion keys are child IDs (`message_id:section_index`). Parent row animation checks must treat active child animations as parent animation, otherwise lazy timeline layout caches the first nearly-collapsed frame.
- Per-file diff measurement and rendering must both interpolate body height from preview to full using the same child-key `tool_expand_progress`; never reserve full height while painting a preview or jump render directly to full while measurement is cached.
- Stable clicked-card anchoring remains on the parent timeline message and must coexist with per-frame interpolated height.
- Focused tests cover bottom-follow request suppression, deliberate pagination, child animation visibility from parent, and clicked-card anchoring. Verified with shared/desktop checks and `cargo build --bin neoism`.
