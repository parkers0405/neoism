---
name: "Composer newline caret fractional scale"
description: "Shift+Enter row capacity lost to f32 floor at fractional scale; epsilon fix, exact-fill terminal caret row, hyphenated model capitalization"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-22"
updated: "2026-10-22"
---

Follow-up after footer cleanup: model label Openai/Gpt-6-astra left suffix lowercase because capitalize_chip_label only split slash/whitespace. Added '-' and '_' word boundaries; display now Openai/Gpt-6-Astra (raw model ID unchanged), tests wide/narrow and extra_high. Shift+Enter correctly inserts newline/cursor bytes and wrap core keeps trailing hard-newline empty rows, but new renderer text_h reconstruction can yield e.g1.99999976 rows at scale0.8/0.95/1.2; strictfloor dropped row, InputViewport revealed final caret by scrolling previous row offscreen, making caret appear stuck at top. Added composer_visible_line_count with +1e-4 dimensionless tolerance beforefloor, still excludes genuinely partialrows. Replaced old binary-exact-only layout test with integrated AgentInputBuffer newline -> wrap -> layout capacity -> InputViewport regression at scales0.6/.75/.8/.95/1/1.2/1.5/2, home/chat, with/without image rail, repeatednewlines beyond5rowcap. Also fixed existing exact-fill softwrap finalization: it flushed occupied row but failed to emit its promised empty successor; final emit now includes trailing_wrap_boundary. Updated wrap_cache.rs exactfill Unicode test and added row-span/EOF tests for blank/hard/repeatednewlines, Unicode, below/exact/overfull, exact-fill+hardnewline. Inputcontroller interiorsoftwrap affinity unchanged. All targeted tests passed: user_input38passed+1ignored microbenchmark; input_controller20; input_viewport4. scoped diffcheck clean. Composer remains bottomanchored so absolute caret screenY can stay constant while card grows upward; correct behavior asserted relative to textviewport, not absolute screenY.
