# Neoism

**A GPU-rendered, terminal-first workspace for code, notes, agents, and multiplayer.**

[![Neoism terminal](https://raw.githubusercontent.com/parkers0405/neoism/241e6daaea1249d2eff6ca94b91dbacc2c426b0f/docs/images/terminal.png)](https://github.com/parkers0405/neoism)

Neoism starts with the terminal instead of hiding it. Real shells, native code and Markdown editors, drawings, AI agents, diagnostics, files, and workspace navigation live together in one Rust-owned interface.

It is not an Electron IDE or a web terminal wrapper. The desktop app owns a native `winit` window and renders through `sugarloaf`; the browser client uses the same renderer family through Rust/WASM and WebGPU/WebGL.

## What Neoism is

- **Terminal-first**: real PTYs, GPU-rendered text, smooth scrollback, tabs, splits, and command navigation.
- **Native editing**: Rust-owned code and Markdown editors with LSP, Vim-style input, a file tree, buffer tabs, diagnostics, and finder.
- **A place for project knowledge**: Markdown, Neoism Notes, Mermaid, notebooks, EPUBs, and `.neodraw` sketches live beside the code.
- **Agent-native**: persistent local agent sessions, parallel sub-agents, LSP, shell and file tools, permissions, checkpoints, undo trees, and durable memory.
- **Multiplayer and remote**: a workspace daemon owns PTYs and shared state so the same workspace can be used from desktop, web, phone, or another laptop over Tailscale.
- **Local-first**: your files, terminals, notes, agents, and credentials stay on machines you control.
- **Discoverable configuration**: `Alt+,` opens the active host's JSONC config with completion for every setting and host-aware suggestions for fonts, agents, models, extensions, and language servers.

## One workspace, one connected system

The workspace daemon keeps terminals, files, collaboration, and remote sessions alive across desktop, web, phone, and other laptops. It supervises Neoism's HTTP/SSE agent server, where persistent sessions, providers, tools, permissions, plugins, MCP, LSP, memory, and sub-agents work together on the same workspace.

## Install

Prebuilt releases support Linux x86_64, Apple Silicon macOS, and Windows x86_64.

### Linux

```sh
curl -fsSL https://raw.githubusercontent.com/parkers0405/neoism/main/scripts/install.sh | bash
```

### macOS

Download the latest DMG from [GitHub Releases](https://github.com/parkers0405/neoism/releases/latest), or use the shell installer command above for a command-line installation.

### Windows

Download and open [`Neoism-x86_64.msi`](https://github.com/parkers0405/neoism/releases/latest/download/Neoism-x86_64.msi), or install it from PowerShell:

```powershell
irm https://raw.githubusercontent.com/parkers0405/neoism/main/install.ps1 | iex
```

The per-user installer requires no administrator rights. Releases include `neoism`, `neoism-workspace-daemon`, and `neoism-agent`. `ripgrep` is recommended for full workspace text search; Neovim is not required.

Update an existing installation with:

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

Documentation ships inside Neoism instead of living in a separate website. Open **Neoism Notes** with `Alt+N` to browse guides for the editor, agent, daemon, multiplayer, extensions, configuration, keybindings, and troubleshooting.

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