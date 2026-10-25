---
name: "Timeline diff clipped line numbers"
description: "Timeline diff cards use Git's measured bold-digit gutter with aligned code/scrollbar; hidden old/new number max cached; tested shared/WASM"
type: "bug"
scope: "project"
origin: "User screenshot of clipped 2218-style timeline diff gutter"
created: "2026-10-23"
updated: "2026-10-23"
---

User screenshot showed agent timeline diff line numbers (e.g. 2218) clipped on the left, same bug previously fixed in the Git review gutter. Root: timeline render_tool_diff_cards still used fixed diff_card::GUTTER_WIDTH=30 through PaintParams::default and body_text_width, while GitDiffView already used diff_card::measured_gutter_width (actual bold digit metrics plus insets).

Fix spans shared panels/agent_pane/view/tool_message{.rs,/diff.rs,/render.rs,/tests.rs}. ToolDiffCardView caches max_line_number across ALL source rows and line_number/old_line_number/new_line_number, not only preview rows. render_tool_diff_cards measures this with the existing shared helper and uses the same gutter in PaintParams, body_text_width_with_gutter for horizontal range, and horizontal scrollbar track origin. Digits remain fixed; code/scrollbar geometry agrees; expanding or vertical scrolling does not change gutter width when hidden rows have more digits. No-wrap rows and heights unchanged. Git review itself is untouched. Four-file patch was 40 additions/2 deletions; no unrelated edits.

Regression timeline_diff_gutter_reserves_hidden_numbers_from_both_sides covers collapsed/expanded caches, narrow/wide panes, four-/six-/ten-digit maxima, old/new/fallback numbers, scales 0.7/1/1.5/2. Existing measured_gutter_keeps_digit_insets_and_code_width_consistent guards padding/geometry. Verified cargo test -p neoism-ui --lib diff (236 passed), markdown_horizontal (1), markdown_scrollbar (2), WASM check --features sugarloaf/web, and git diff --check. Full native --tests check blocked by concurrent unrelated native edits (missing hydrate_generated_images, CodePane diagnostic_summaries access, SessionPreloaded state fields, PanelFrameRef migration and Screen constructor fields). These files were NOT modified/reverted; no need to re-investigate their errors as a timeline bug. No live GPU/screenshot validation, app launch, release build or commit.
