# Terminal, Editor, and Notes

Neoism keeps three complementary views of a project close together: run things in the terminal, change source files in the editor, and preserve project knowledge in Markdown notes.

## Terminal

Create a terminal tab with `Ctrl + Shift + T` (`Cmd + T` on macOS). New tabs begin in the active workspace directory and run the configured shell as a real PTY, so interactive programs, job control, mouse reporting, OSC links, and alternate-screen applications work normally.

Useful terminal controls:

- `Shift + PageUp` / `Shift + PageDown` scroll a page.
- `Shift + Home` / `Shift + End` move to the top or bottom of scrollback.
- `Ctrl + =`, `Ctrl + -`, and `Ctrl + 0` change or reset font size (`Cmd` equivalents on macOS).
- **Copy**, **Paste**, **Clear**, and **Search** are available from the terminal context menu.

A shell's current directory is local to that terminal. It does not move the workspace root or the file tree.

## Editor

Toggle the file tree with `Alt + E`, then open a file. Neoism's native editor provides line numbers, buffer tabs, selection, undo/redo, project search, diagnostics, and language-server actions. Markdown uses the same editor infrastructure with rendered headings, links, lists, code fences, tables, task boxes, and other document structure.

The optional Vim layer is controlled by `editor.vim-mode` and can also be toggled with `Alt + Shift + Space`. The command palette exposes editor actions even when they have no dedicated keybinding.

Use `Ctrl + Shift + Left` / `Ctrl + Shift + Right` for the previous or next editor buffer tab (`Ctrl + Shift + [` / `]` are also available). Editor changes are ordinary changes to files in your workspace and remain visible to Git and shell tools.

## Notes

Press `Alt + N` to open the notes sidebar. A notes **vault** is an ordinary folder of Markdown files; Neoism indexes its hierarchy, links, tags, headings, backlinks, and tasks without converting it to a proprietary database.

Notes support familiar wiki-style navigation:

- `[[Page Name]]` links to another page;
- `[[Page Name#Heading]]` links to a heading;
- `[[@project/Page]]` links into a vault associated with another project;
- `#tags` are indexed for discovery;
- `- [ ]` and `- [x]` remain plain Markdown tasks.

Neoism can link the current workspace to a notes vault. The relationship is recorded in `.neoism/workspace.json`, while the vault's `project.json` records associated code directories for project-aware page completion. Your note content itself stays as Markdown.

Drawings use Neoism's `.neodraw` format and can be embedded from Markdown with a fenced `draw` block that references the drawing file.

## Put them together

A practical layout is a source file beside a terminal, with notes available in the sidebar. Use `Alt + Arrow` to move focus between panes and panels. This gives an agent the same stable project boundary while you keep commands, implementation, and decisions visible.

Next: [[04 Start Your First Agent|Start Your First Agent]].