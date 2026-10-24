---
name: "Hover copy routing and blue underlined links"
description: "Explicit Copy now routes hover -> active code -> terminal across all native entry points; configured uppercase keys normalized; hover links use readable blue and clipped underlines."
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-21"
updated: "2026-10-21"
---

Explicit desktop Copy must resolve the visible matching LSP hover selection before code-buffer or terminal selection. Central resolver is `screen/selection/selection_ops.rs::copy_selection`: ordinary Clipboard copies lazily choose `code_hover_selected_text()` -> active_code_pane().buffer.copy_payload() -> terminal selected_text. PRIMARY/ClipboardType::Selection stays terminal-only and mouse-selection behavior is not a substitute for explicit Copy; do not enable auto-copy as a workaround. Configured Act::Copy, early Super+C, Ctrl+Insert and palette Copy all call this path. The original raw Ctrl/Cmd+C check in code/input.rs was too late for early consumers and lowercase-only.

Binding configuration canonicalizes character triggers to lowercase. Normalize the key after the key_without_modifiers branch too, since Shift/Caps/Alt paths can still produce uppercase; this covers Ctrl+Shift+C and configured non-C Copy actions. The code editor's raw hover-copy fallback is case-insensitive. `code_hover_selected_text()` must use active_code_pane (including embedded editors), match path/cursor ownership, require visible popup bounds, and reject competing finder/palette/modal/settings/browser/context-menu/navigator/diagnostic/completion/action overlays. Hidden/stale selection must not steal Copy. Tests use lazy payload callbacks that panic if editor or PTY fallback is invoked for a hover copy; no actual OS clipboard or GUI is needed.

Hover link text must use `theme.readable_accent(theme.blue)`, not theme.accent: some themes use near-white accent, which made working hyperlinks indistinguishable from prose. DrawOpts has no underline field; `panels/hover_popup.rs::link_underline` computes clipped geometry per measured wrapped link piece, excluding inline-code chip padding, and the renderer paints it in the same readable blue. Only real safe HTTP(S) link spans get styling. Pulldown-cmark already resolves full/collapsed/shortcut reference links when definitions are present anywhere in the complete document and supports angle autolinks; unresolved references/unsafe URLs must never get fabricated destinations. Fixtures cover references, nested emphasis/inline-code labels, white accents, underline clipping/wrapping and unchanged canonical copied text.
