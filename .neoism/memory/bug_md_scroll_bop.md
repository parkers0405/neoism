---
name: bug_md_scroll_bop
description: "MD scroll 'bounce' when HOLDING arrow up/down = Live-Preview reveal re-measures the cursor line each row it sweeps"
metadata: 
  node_type: memory
  type: project
  originSessionId: d3ba8817-3b2e-4d6c-84cd-4fad8c2df2e4
---

Symptom (user, precise): HOLDING arrow up/down makes the **blocks below the caret bounce ~one row on every keystroke**; single presses are fine; touchpad is okay; way worse on formatted docs than plain paragraphs.

Real root cause: **Live Preview reveal.** The caret's line is laid out from its RAW markup (`draw_blocks.rs::measure_item` → `inline_wrapped_lines_raw`/`plain_inline_words_for_text`), every other line from the rendered form. Raw has more chars (`**`, `` ` ``, `[..](url)`), so a marked-up line wraps to an extra row WHILE the caret is on it and collapses when it leaves. Plain text wraps identically raw or rendered → no bounce (matches "plain docs barely do it"). As a held arrow sweeps line-by-line, each line grows then shrinks → everything below bounces a row per keystroke. Node heights come ONLY from `measure_item` (committed via `CommitMeasuredLayouts`); the draw never sets height.

Fix (landed): suppress the raw reveal **for measurement only** during a fast key-repeat stream, so layout heights stop changing mid-scroll. `MarkdownVirtualRenderState` gains `last_cursor_change_at` + `cursor_reveal_suppressed`, set in `render_virtual`'s cursor-line-change block (`surface.rs`): if two cursor-line changes land < `CURSOR_REVEAL_FAST_REPEAT` (90ms) apart it's a stream → suppress. `commit_visible_measurements` gates `cursor_inside` (height + cache `cursor_token`) on `pane.cursor_reveal_active()`, which returns false until the caret has been still ≥ `CURSOR_REVEAL_SETTLE` (90ms). `tick_scroll` keeps requesting frames while a reveal is pending so it pops back in right after you let go. The DRAW and CARET paths are untouched (caret still maps raw via the wrap rows draw builds) → no caret-drift risk; cost is a brief overpaint of the revealed extra row during the hold that re-measures clean on settle, plus one small height settle when you stop (2 tiny shifts bracketing a stable hold vs N bounces).

Also landed (complementary, `scroll_cursor_into_view`): re-base the caret to its TARGET-scroll position before the scrolloff math — `let pending = target_scroll_y - scroll_y; let y = y - pending;` — so the held-scroll nudge doesn't over-shoot off the lagging animated scroll_y. Single press identical (pending≈0).

DEAD END (made it worse, reverted): auto scroll-anchoring (35% pin) in `commit_visible_measurements` every `layout_changed` frame — it re-pinned a node mid-measurement, injecting that node's own estimate→real delta as a NEW jump on downward scroll. Don't anchor against a node being measured this frame.

See [[project_markdown_editor]] (virtualized renderer, caret↔rendered mapping is fragile); [[feedback_keep_building]] (verify visual md fixes with the user's eyes).
