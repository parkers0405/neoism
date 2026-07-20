# Getting Started

Welcome to **Neoism** — a terminal-first workspace for code, notes, agents, and multiplayer editing.

Neoism starts from the terminal instead of hiding it. Shells, a native code editor, Markdown notes, drawings, AI agents, file trees, and command palettes all live inside one GPU-rendered chrome layer — on the desktop app, in a browser, or on your phone over Tailscale.

This `Welcome` folder is your built-in guide. Open it any time from the **Getting Started** button on the start screen, or press **Alt + N** for the notes sidebar. Links below are wiki links — click one to jump.

## The four pillars

- [[The Terminal]] — real shells with GPU-rendered text, smooth scrollback, tabs, and a command palette from any focus.
- [[Editor/The Editor|The Editor]] — Neoism's native code editor, with [[Editor/Languages and LSP|language servers]] that install on demand.
- [[The Neoism Agent]] — a local agent server with sub-agents, a full tool runtime, permissions, checkpoints, and memory.
- [[Notes and Drawings]] — this vault, Markdown with wiki links and backlinks, and hand-drawn `.neodraw` sketches.

And when one screen isn't enough: [[Multiplayer]].

## First steps

1. **Command palette** — `Ctrl + P` (or `Cmd + ;` / `Cmd + :`). Almost everything is reachable here, including actions with no keybinding.
2. **File tree** — `Alt + E` to browse your project.
3. **An agent** — `Alt + A`, or type `:claude`, `:codex`, or `:opencode` in a terminal.
4. **Search** — `Alt + S`.

The full list is in [[Keybindings]].

## Make it yours

- [[Configuration/Configuration|Configuration]] — the `config.json` reference.
- [[Configuration/Themes, Cursor and Fonts|Themes, Cursor & Fonts]] — pick a theme, set your cursor color, choose a font.
- [[Configuration/Shaders|Shaders]] — optional CRT and post-process filters.

## Where things live

- Your **notes vault** is a plain folder of Markdown files (this one is under `~/Neoism/Vaults/`). Edit it here or in any other editor — it's just files, easy to sync with Git, iCloud, Dropbox, or Syncthing.
- **Config** is one file: `~/.config/neoism/config.json` — terminal, editor, *and* agent settings together, JSONC (comments welcome), hot-reloaded on save (see [[Configuration/Configuration|Configuration]]).
- **Project state, sessions, and pairing** are owned by `neoism-workspace-daemon`, which lets the same workspace open on the desktop, a browser, or a phone.

Everything you see is drawn by Neoism's own engine — not a browser, not a widget toolkit. Poke around.
