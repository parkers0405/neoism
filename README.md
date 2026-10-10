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

Every release includes Linux, macOS, and Windows builds. Download the latest from [GitHub Releases](https://github.com/parkers0405/neoism/releases/latest).

Cross-play is supported: people on different platforms can join the same workspace.

## Build from source

```sh
git clone https://github.com/parkers0405/neoism.git
cd neoism
cargo build --bin neoism
./target/debug/neoism
```

## Documentation

Documentation ships inside Neoism. Open **Neoism Notes** with `Alt+N` for the editor, agent, daemon, multiplayer, extensions, configuration, keybindings, and troubleshooting.

Native UI, commands, keymaps, events, panels, agent surfaces, trees, tabs, status, and chrome are customizable from [`~/.config/neoism/init.lua`](docs/lua.md) without moving rendering out of Rust.

Plugin authors and coding agents should start with the canonical [`plugin architecture and package guide`](docs/plugins.md), then use the complete [`editor plugin API`](docs/editor-plugin-api.md), [`Agent plugin API`](docs/agent-plugins.md), and generated [`Lua annotations`](docs/lua-api.lua). These references cover every exposed tier, manifest field, capability, contribution, lifecycle rule, protocol frame, broker and security boundary.

For standalone Agent, product embedding, and cloud workers, start with the [Agent runtime README](neoism-agent/README.md) and the authoritative [hosted architecture and implementation status](neoism-agent/docs/hosted-control-plane.md).

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
| [`neoism-cloud-runtime`](neoism-cloud-runtime/README.md) | Provider-neutral whole-workspace lifecycle, durable registry, and v2 infrastructure bridge |
| [`neoism-cloud-host`](neoism-cloud-host/README.md) | Workspace launch manager, controller signing, readiness verification, connection broker, and policy-injected host API |
| `neoism-terminal-core` | Terminal parser, grid, selections, and effects model |
| `sugarloaf` | Native and web GPU rendering |
| `neoism-protocol` | Wire types shared by clients and the daemon |

## Contributors

- [Logan Settle (@LoganSettle)](https://github.com/LoganSettle) - Windows MSI upgrade and repair fixes.

Neoism is open source under the [MIT License](LICENSE). See [NOTICE](NOTICE) for third-party attribution.

Join the [Neoism Discord](https://discord.gg/FF2KUFMRd).
