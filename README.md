# Neoism

**A terminal-first IDE for code, agents, and multiplayer.**

[![Neoism](https://raw.githubusercontent.com/parkers0405/neoism/241e6daaea1249d2eff6ca94b91dbacc2c426b0f/docs/images/terminal.png)](https://github.com/parkers0405/neoism)

Neoism is a GPU-rendered, local-first IDE that starts with the terminal instead of hiding it. Real shells, a native code editor, Markdown notes, drawings, Git, language servers, and a full agent runtime share one workspace. A workspace daemon keeps that workspace alive so desktop, web, phone, and other laptops can join it.

It is not an Electron IDE and not a chat window bolted onto a terminal. The desktop app owns a `winit` window and renders through `sugarloaf`. The browser client uses the same renderer family through Rust/WASM and WebGPU/WebGL.

## Terminal

The terminal is the center of the workspace, not a drawer under an editor:

- Real PTYs with GPU-rendered text, smooth scrollback, tabs, splits, and command navigation
- Interactive programs, job control, mouse reporting, OSC links, and alternate-screen apps
- New tabs start in the workspace directory; a shell `cd` stays local to that pane

## Editor

Rust-owned buffers for source and Markdown, not an embedded nvim or a web editor:

- File tree, buffer tabs, splits, finder, and project search
- Tree-sitter highlighting and a managed LSP catalog (hover, go-to, references, symbols, diagnostics, format, code actions)
- Optional Vim layer
- Git diff panel for status, staging, commits, and branches
- Native Markdown with wiki links, live preview, notebooks, EPUB reading, and `.neodraw` sketches
- Settings GUI and a command palette (`Alt+P`) with completion for every action

## Agent

Neoism ships its own agent server, not a hosted chat iframe. Sessions belong to the workspace and survive closing a pane or reconnecting from another device.

- Persistent conversations over a versioned HTTP/SSE API
- Providers, models, skills, MCP, plugins, and a TypeScript SDK
- Shell, file, patch, search, LSP, notes, and web tools with an ask/allow/deny permission model
- Parallel sub-agents (`explore`, `general`, and custom agents), checkpoints, undo/redo, compaction, and durable memory
- Open an agent pane with `Alt+A`

## Multiplayer and remote

The workspace daemon is the authority for files, PTYs, layout, pairing, and shared editor documents:

- Live co-editing through daemon-owned CRDT documents, plus presence carets and avatars
- The same terminals and agent sessions from desktop, browser, or phone
- Join another machine over a private network or Tailscale; promote a workspace to a different host when you mean to move it
- Local-first: files, terminals, notes, credentials, and agent history stay on machines you control

## Notes

Notes are ordinary Markdown vaults with graphs, backlinks, tags, and tasks (`Alt+N`). Drawings and notebooks live next to the code they describe.

## Install

Prebuilt releases support Linux x86_64, Apple Silicon macOS, and Windows x86_64.

### Linux

```sh
curl -fsSL https://raw.githubusercontent.com/parkers0405/neoism/main/scripts/install.sh | bash
```

### macOS

Download the latest DMG from [GitHub Releases](https://github.com/parkers0405/neoism/releases/latest), or use the shell installer above.

### Windows

Download [`Neoism-x86_64.msi`](https://github.com/parkers0405/neoism/releases/latest/download/Neoism-x86_64.msi), or from PowerShell:

```powershell
irm https://raw.githubusercontent.com/parkers0405/neoism/main/install.ps1 | iex
```

The per-user installer requires no administrator rights. Releases include `neoism`, `neoism-workspace-daemon`, and `neoism-agent`. `ripgrep` is recommended for workspace text search.

```sh
neoism update
```

## Build from source

```sh
git clone https://github.com/parkers0405/neoism.git
cd neoism
cargo build --bin neoism
./target/debug/neoism
```

## Documentation

Documentation ships inside Neoism. Open **Neoism Notes** with `Alt+N` for the editor, agent, daemon, multiplayer, extensions, configuration, keybindings, and troubleshooting.

A first tour: open a project, `Alt+E` for the file tree, `Ctrl+Shift+T` for a terminal, `Alt+A` for an agent, `Alt+P` when you do not know the command.

## Architecture

| Path | Role |
|---|---|
| `neoism-frontend/desktop` | Native `neoism` app, window host, and desktop integration |
| `neoism-frontend/shared` | Shared UI, panels, layout, editors, and interaction policy |
| `neoism-frontend/wasm` | Rust terminal and chrome renderer for the browser |
| `neoism-frontend/web` | TypeScript web host and daemon client |
| `neoism-workspace-daemon` | PTYs, workspaces, pairing, remote sessions, and shared state |
| `neoism-agent` | Agent server, CLI, providers, tools, permissions, and memory |
| `neoism-terminal-core` | Terminal parser, grid, selections, and effects model |
| `sugarloaf` | Native and web GPU rendering |
| `neoism-protocol` | Wire types shared by clients and the daemon |

Neoism is open source under the [MIT License](LICENSE). See [NOTICE](NOTICE) for third-party attribution.
