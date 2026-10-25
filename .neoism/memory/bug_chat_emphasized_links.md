---
name: "Chat links inside bold lead-ins — fixed"
description: "Bold chat links containing a colon/prose were swallowed as opaque styled text; recursive emphasis parsing preserves link targets and wrapping."
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-18"
updated: "2026-10-18"
---

Chat showed literal `[label](url)` in bold bullet lead-ins while nearby ordinary links rendered blue. Reproduced with `**[OpenCode Mobile by Alvaro Lorente](https://getopencode.app/):** Details.`: old exact-link emphasis special case required `**` immediately after `)`, so the colon caused fallback to opaque Bold text. Fixed `neoism-frontend/shared/src/panels/agent_pane/view/markdown/inline_style.rs` by recursively parsing emphasis contents (depth bounded at 32), inheriting bold/italic on structured MarkdownLink segments, and skipping whole links/code/escapes when scanning for closing emphasis markers. Replaced narrow emphasized_markdown_link helper. Preserves click targets through inline_wrap_tokens serialization/reparse. Tests in adjacent markdown/tests.rs cover all three screenshot links, Unicode labels, trailing colon, surrounding prose, all six emphasis marker forms plus unstyled, and wrapped-link target/style round trips. Regression failed before fix; all 37 chat Markdown tests passed afterward. Exact source from screenshot was not retrieved, but symptom reproduced by bold lead-in containing colon.
