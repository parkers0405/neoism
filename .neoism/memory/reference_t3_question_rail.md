---
name: "T3 Code question-rail reference"
description: "Public T3 source reference: one tick per user turn, final-answer hover card, 8px spacing and hover fisheye; NOT density minimap"
type: "reference"
scope: "project"
origin: "neoism-agent"
created: "2026-10-23"
updated: "2026-10-23"
---

User corrected agent rail: wants T3 Code's QUESTION/TURN NAVIGATOR, not a fixed 11-band density minimap. Parent inspected public source and cloned sparse checkout at `/tmp/neoism-t3code-question-rail` (git revision `9a391eb3f55b738113b61768cf2d8263b4f3ac2e`). Relevant upstream files: `apps/web/src/components/chat/TimelineMinimap.tsx`, `timelineMinimapItems.ts`, `MessagesTimeline.logic.ts`, `MessagesTimeline.tsx`.

T3 behavior: `deriveTimelineMinimapItems` emits one item per projected USER row, stores row index/question text and FINAL assistant text before next user, ignoring work/tool rows. Hover card question one line ellipsized, final assistant markdown-to-plain excerpt max3 lines; only hovered answer parsed, bounded input1500. Rail min2 items, natural spacing8px, height capped by available viewport, evenly spaced by question index, not answer length/content offset. Hover fisheye scales hovered1.0, adjacent0.667, distance2=0.417, rest0.333;150ms transitions. Highlight user rows currently in viewport; navigation current index first visible question else preceding question being read. Click scrolls to question's row, not proportional history. T3 original is left; Neoism user explicitly wants RIGHT gutter. Keep rail ink/hits out of chat, place preview to LEFT and properly mask underlying glyphs. Source includes prev/next arrows, keyboard navigation and selectable preview, but primary user scope question ticks+hover+click.

Do not reintroduce arbitrary density weighting: earlier `timeline_scroll_rail_profile` was a misunderstanding, to be replaced by real question targets and logical anchor navigation.
