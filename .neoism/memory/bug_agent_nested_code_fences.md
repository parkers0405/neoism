---
name: "Agent nested code fences — FIXED"
description: "Rare empty agent code cards caused by unconditional fence toggling; fixed with marker/length-aware CommonMark fence tracking."
type: "bug"
scope: "project"
origin: "coding session"
created: "2026-08-01"
updated: "2026-08-01"
---

# Agent code cards split by nested fences — FIXED

The rare agent-chat rendering failure where empty one-line code cards appeared while intended code rendered as prose was caused by the custom shared Markdown parser treating every line beginning with three backticks or tildes as an unconditional fence toggle. It retained neither opening marker type nor run length. An outer four-backtick Markdown example containing inner triple-backtick examples therefore closed early, shifted parser state, emitted empty code cards, and painted code as prose.

Fixed in `neoism-frontend/shared/src/widgets/markdown.rs` and `neoism-frontend/shared/src/panels/agent_pane/view/markdown.rs` by adding `FenceDelimiter`/`fence_open`, enforcing CommonMark-style opener indentation and matching closers by marker and minimum run length. The same tracker now drives semantic line joining, agent code-card extraction, multiline-link normalization, and `code_block_end`.

Regression tests cover outer four-backtick blocks with multiple inner triple fences, mixed tilde/backtick fences, link normalization inside nested fences, closer validity, and shared block-end matching. Verified with `cargo check -p neoism-ui`, 33 agent Markdown tests, focused fence tests, rustfmt check, and git diff check.
