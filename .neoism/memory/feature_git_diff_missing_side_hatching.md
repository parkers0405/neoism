---
name: "Git diff missing-side diagonal hatching"
description: "Missing-side diagonal hatching: split-only blank placeholders, merged clipped blocks, ordered triangles required over card backgrounds"
type: "feature"
scope: "project"
origin: "screenshot request"
created: "2026-10-21"
updated: "2026-10-21"
---

User requested Zed diagonal stripes on missing side of split diffs. Implemented in shared widgets/diff_card.rs with PaintParams.hatch_empty_rows (default false, aggregate GitDiffView sets split). Missing row = Context + empty text + no line_number/old_line_number/new_line_number; real blank source lines have coordinates and must not hatch. Merge adjacent visible padding rows into one block, start after gutter, draw subtle surface tint + phase-aligned clipped 45-degree segments. Use sugarloaf.triangle_ordered for stripes at base_order+3: plain sugarloaf.line() is compositor order 0 and would disappear beneath opaque card backgrounds. Inset stripe geometry so strokes cannot leak into gutters, sticky headers, or occluded regions. New tests distinguish real blank lines, merge runs, and verify line clipping/phase at multiple scales. Earlier 34 Git-related + 10 diff-card tests passed before this addition; latest compile/test attempts blocked by concurrent unrelated HoverPopupStyle::opts/parse/cache-key signature changes in panels/hover_popup.rs. Git modifications themselves reported no compile errors. Don't modify another agent's hover popup changes to force verification. See feature_git_review_geometry.md for scrolling/gutters/sticky header/tree restore.
