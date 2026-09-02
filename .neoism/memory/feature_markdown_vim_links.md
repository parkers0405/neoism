---
name: "Markdown Vim links and phone calls"
description: "Markdown mouse Visual-to-Normal behavior and Normal-mode Enter link/tel activation"
type: "feature"
scope: "project"
origin: "Markdown editor implementation"
created: "2026-08-07"
updated: "2026-08-07"
---

# Markdown Vim mouse selection and link activation

- Mouse drag creates Markdown Visual selection. A subsequent plain click that collapses the selection must call `enter_normal()`, never `enter_insert()`. Implemented in `MarkdownPane::end_drag`; original drag release remains Visual because its range is non-empty.
- `MarkdownPane::link_at_cursor()` resolves links from raw source cursor bytes for Normal-mode Enter:
  - `[[Note]]` and code refs -> internal targets resolved through `resolve_markdown_link`.
  - `[label](relative.md)` -> internal Markdown target.
  - `[label](https://...)`, bare HTTP(S), mailto, and tel -> external target.
- Desktop plain Enter in Normal mode now opens a cursor link first; if no link exists it preserves existing task-checkbox toggle behavior.
- `open_markdown_link_target` recognizes `http://`, `https://`, `mailto:`, and `tel:` as external and passes URI unchanged to `background_process::open_url` (macOS `open`, Linux `xdg-open`, Windows ShellExecute).
- Phone syntax: `[Call](tel:+15551234567)` or bare `tel:+15551234567`. Prefer E.164 (`+` plus country code and digits); OS decides the dialer/continuity app. Neoism does not place a call directly.
- Shared markdown widget link parsing now includes `tel:` and `mailto:` as bare external schemes.
- Focused mouse/link/tel tests, all 46 Markdown Vim tests, shared/desktop cargo checks, and `git diff --check` pass.
