---
name: "Agent timeline diff horizontal scrolling"
description: "Timeline diffs no longer wrap; reuse fenced-code horizontal scrollbar, retain hide/show-more and fixed line-number gutter"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-13"
updated: "2026-10-13"
---

Agent timeline diff cards now opt into no-wrap horizontal scrolling; Git panel retains wrapped default. `widgets/diff_card.rs` adds `CardSpec.horizontal_scroll: Option<f32>` (`None` wraps, `Some(offset)` shifts only code with fixed gutter), full tab-expanded content-width measurement, clipped complete syntax tokens without ellipses, and logical-row fallback. `tool_message/diff.rs` builds 0..=line-count offsets and preserves preview/show-more caps; views cache measured content width. `AgentToolPane::code_scroll_pane` exposes existing `AgentMarkdownPane` scrolling hooks for both hosts. Timeline uses namespaced `diff:<message-id>:<section-index>` state and reuses Markdown wheel/drag/sticky hover scrollbar; pointer is forwarded through message_card delegate. Vertical expansion viewport, collapse/hide controls, links, and diagnostics remain unchanged. Regression coverage includes single-row long source lines at narrow widths, fixed preview height on show-more, per-file offsets, resize clamps, drag vs collapse, and lossless tab expansion. Final verification passed: 61 diff regressions, 1 markdown_horizontal test, 2 markdown_scrollbar tests, `cargo check -p neoism`, and `cargo check -p neoism-terminal-wasm --target wasm32-unknown-unknown`. Not visually verified in running application.
