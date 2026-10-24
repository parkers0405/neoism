---
name: "Agent right-side question rail"
description: "T3-style question navigator replaces density minimap: one tick/user turn, final-answer hover card, semantic anchored jumps, right gutter, desktop/WASM checks pass"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-23"
updated: "2026-10-23"
---

The agent timeline rail is now a T3-style QUESTION/TURN NAVIGATOR, superseding the old fixed11 density minimap and its `timeline_scroll_rail_profile` (removed). User explicitly corrected by screenshot and requested inspecting T3 source. Reference sparse clone `/tmp/neoism-t3code-question-rail`, revision9a391eb3f55b738113b61768cf2d8263b4f3ac2e; details in [[reference_t3_question_rail]].

One tick per displayed projected User row, min2 questions, no11 cap. Natural pitch8*s, vertically centered and capped by metrics viewport height. Existing right gutter only (8px narrow/20px normal, maxtrack26), no transcript width change. Hover fisheye ratios1,2/3,5/12,1/3 by distance; left edge fixed so marks extend right. Active question is first visible question else preceding question being read, theme fg0.9; hover muted0.75/othersmuted0.35. Tick width is NOT based on answer density. No continuous painted track/proportional thumb.

Shared data/hit policy `panels/agent_pane/question_rail.rs`: TimelineQuestionTarget {source_index,message_id,top,assistant_source_index}, TimelineQuestionTargets=Rc<Vec<...>>, question_index_at(track,count,pointer). `TimelineLayoutCache.question_targets` rebuilt only on layout work by `view/timeline/render.rs::question_targets`, reused cheapRc otherwise. Last Assistant before next User is answer excerpt; reasoning/tools/system don't terminate a turn. `render_timeline_in_paint_viewport_with` accepts optional question gutter and returns Option<TimelineQuestionRailFrame>; chat paints exact same-frame snapshot after restoring transcript masks.

Preview: one measured ellipsized user line, up to3 measured answer lines, bounded1500chars before HTML safety projection/Markdown-to-plain parsing, only hovered turn parsed. Image-only fallback. Rounded theme overlay left of rail, pane-clamped. Register preview in renderer2D content mask and hit mask BEFORE transcript paint, restore before overlay draw. IMPORTANT do NOT append preview rect to legacy text occlusion slices: those hide entire lines outside card. overlay_text_mut().draw is real API (draw_ui_tooltip doesn't exist).

Shared/native state stores Rc targets with owning session ID; setter via AgentTimelinePane and macro adapters. Existing scrollbar_hit/press/drag bridge now resolves nearest QUESTION by real count, jumps to target.top, cancels follow/motion, sets TimelineViewAnchorKey::for_source at screen offset0 before lazy remeasurement. Resolve stale source indices by id and reject non-User targets/session mismatches. Missing geometry clears targets/drag. Legacy geometry-only helpers remain for existing consumers/tests. Shared TimelineScrollbarHover::update_questions uses semantic count; host cursor/pointer-capture still works, right rail stays hand cursor. Web pointer leave passes None for mouse.

Verification latest: `cargo check -p neoism -p neoism-ui --tests --message-format short` AND `cargo check -p neoism-terminal-wasm --target wasm32-unknown-unknown --message-format short` passed on0.7.135; scoped git diff --check passed. Tests compile only, not run/app not launched (user owns rebuild/relaunch). New tests cover >11 counts, user/final-answer projection, fisheye, gutters, preview sanitization/measurement/masks, unequal answers, anchor remeasure, prepends, non-user/duplicate/empty IDs and session cleanup. Earlier web bridge tests11/typecheckpassed remain unaffected.
