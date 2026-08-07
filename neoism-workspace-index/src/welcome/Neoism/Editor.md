# Editor

Neoism includes a native editor rendered by the same engine as the terminal. It supports source files, Markdown, drawings, notebooks, syntax highlighting, Vim-style editing, and language-server features without embedding a browser editor.

## Open and save files

Open Explorer with `Alt+E` and select a file, or use Finder/project search. Files open in buffer tabs inside the active workspace. Dirty tabs show modified state; normal close commands refuse to discard unsaved content.

Saving writes through Neoism's buffer/daemon path so connected clients receive the saved state. Git and terminal tools see ordinary host files.

## Buffer tabs and splits

Buffer tabs belong to the active workspace/pane. Move among them with the configured previous/next buffer actions, drag them between panes, or split right/down. Closing a split-local dirty tab has the same protection as a workspace-level tab.

## Vim mode

Toggle the Vim layer with `Alt+Shift+Space` or configure it:

```jsonc
{
  "editor": {
    "vim-mode": true
  }
}
```

Neoism supports normal/insert/visual workflows, counts, motions, operators, search, command-line actions, and common Ex commands. `:q`, `:q!`, `:qall`, and `:qall!` follow Neoism's buffer/workspace scopes described in [[Workspaces]].

## Syntax and language intelligence

Syntax parsers are built into Neoism for supported languages. LSP servers provide hover, definitions, implementations, references, symbols, diagnostics, highlights, folding, selection ranges, formatting, code actions, and call hierarchy.

Install servers from **hamburger menu → Extensions → Language Servers**. The same managed registry powers missing-server prompts and the Agent's LSP tools. Extensions install into Neoism's managed directory rather than requiring global package-manager changes.

Live server state appears in the editor status area. A buffer may attach to multiple servers, such as a linter plus a type checker.

## Diagnostics and actions

Diagnostics appear in the editor and status UI. Open a diagnostic to inspect its message; use code actions where the server provides them. Agent tools can query the same server state, but edits still pass through normal file and permission paths.

## Formatting

Formatting is language-server-driven for supported buffers. Install/attach the language server first. A server that does not advertise formatting cannot produce formatting edits.

## Markdown, drawings, and notebooks

Markdown uses Neoism's native Markdown renderer/editor with headings, links, lists, code fences, tables, tasks, and live-preview behavior. `.neodraw` files open the drawing editor. Supported notebook files open notebook surfaces with cell execution routed through configured kernels.

## Collaborative editing

Editor buffers use daemon-authoritative collaborative document state. Connected clients receive content and presence updates. Not every web/mobile Markdown surface is a full co-editing peer; see [[Neoism Daemon/Multiplayer and Sync]].

## Advanced custom LSP

Most users should use Extensions. Custom server routes can be defined under the Agent `lsp` block when the managed catalog does not cover a server. See [[Neoism Agent/Formatters, LSP, and References]].

## Troubleshooting

- Missing intelligence: install the matching server in Extensions and reopen/touch the file.
- Stale diagnostics: save or touch the document so the server receives current content.
- Dirty close refused: save, or use the explicit bang form only when discarding is intended.
- A server that repeatedly exits should be inspected from Extensions/status and its process logs.