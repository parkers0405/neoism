---
name: "Question custom-answer spaces and wrapping - fixed"
description: "Custom answer keeps trailing spaces, wraps with measured widths and graphemes, clips caret/text; MCP icon feasibility notes, not implemented"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-16"
updated: "2026-10-16"
---

Fixed question picker's editable custom answer dropping visible spaces and overflowing field. `shared/src/widgets/inline_picker.rs::wrap_prompt_text` normalized whitespace via split_whitespace; state/question_policy retains raw question.typed but the reconstructed last line lost trailing spaces, so caret didn't advance until next letter. Added separate wrap_answer_text for editable query_lines only: whitespace-preserving split_word_bounds, actual full-candidate text measurement, grapheme-safe oversized-run splitting, explicit blank lines. Readonly prompt/title/MCP activity keep old normalized wrapper. Question text now clips to inner answer bounds, caret clamps/intersects with same field via answer_caret_rect. Existing five-line tail viewport/header row-hit geometry retained. Added regressions for leading/repeated/trailing spaces, whitespace-only/trailing newline, proportional widths, long emoji/tokens, and caret bounds across scales. `cargo check -p neoism-ui --tests` and wasm32 check passed; tests compiled, not executed; no live UI by agent.

Separate user question (NOT implemented): modern MCP 2025-11-25 supports optional serverInfo.icons (Icon src/mimeType/sizes/theme), including local stdio servers. Official schema https://modelcontextprotocol.io/specification/2025-11-25/schema#implementation . Existing mcp_runtime initialize_client currently requests protocol2024-11-05, keeps capabilities only and discards serverInfo; McpCatalogEntry has no icons. Proper icon implementation needs capture/store metadata and expose catalog/icon identity, nonblocking bounded fetch/decode + disk/memory/texture cache, consistent icons in list/authdetail, fallback known-brand asset/config override then generic Nerd Font plug/server. Do not connect disabled servers merely to retrieve icons or fetch live favicons during render. Not all providers advertise icons; do not claim Firecrawl currently does without checking.
