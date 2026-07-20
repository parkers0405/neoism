# The Editor

Neoism ships its **own native code editor** — built in Rust, rendered by the same engine as the rest of the workspace, and styled after a tuned nvim setup: line-number gutter, cursorline, `~` past the end of the buffer, modal editing if you want it.

- [[Languages and LSP|Languages & LSP]] — syntax and language servers.

## Opening files

- **Alt + E** toggles the file tree; click a file (or press Enter) to open it.
- **Alt + S** opens project search.
- The **command palette** (`Ctrl + P`) has a fuzzy file finder — start typing a filename.

## Editing your way

Standard editing works out of the box — arrows, Shift-select, `Ctrl+C/X/V`, `Ctrl+Z/Y`, Tab/Shift-Tab indent, smart Home. Prefer modal editing? Toggle **Vim mode** from the palette and you get operators (`d`, `c`, `y`, `>`), motions (`w b e f t % gg G { }`), text objects (`iw`, `i"`, `i(`, `ip`), Visual mode, registers via the system clipboard, and dot-repeat.

## Buffer tabs & panes

Open files show as buffer tabs. Move between them and split the view:

```text
Ctrl+Shift+Left / Right     previous / next buffer tab
Ctrl+Shift+R                split right
Ctrl+Shift+D                split down
```

(macOS uses `Cmd`; see [[../Keybindings|Keybindings]].)

## Diagnostics

Errors and warnings surface as underlines in the buffer, inline chips at the end of the line, and a status-bar pill. Syntax highlighting (Tree-sitter, whole-buffer accurate) is built in; language intelligence (LSP) installs on demand — details in [[Languages and LSP|Languages & LSP]].

The goal isn't a fake VS Code — it's a fast, native editor with enough chrome that navigation, diagnostics, agents, and notes feel like one workspace.
