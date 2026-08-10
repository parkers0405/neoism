# Neoism

**A GPU-rendered, terminal-first workspace for code, notes, agents, and multiplayer.**

[![Neoism terminal](https://raw.githubusercontent.com/parkers0405/neoism/241e6daaea1249d2eff6ca94b91dbacc2c426b0f/docs/images/terminal.png)](https://github.com/parkers0405/neoism)

Neoism starts with the terminal instead of hiding it. Real shells, managed Neovim panes, Markdown notes, drawings, AI agents, diagnostics, files, and workspace navigation live together in one Rust-owned interface.

It is not an Electron IDE or a web terminal wrapper. The desktop app owns a native `winit` window and renders through `sugarloaf`; the browser client uses the same renderer family through Rust/WASM and WebGPU/WebGL.

## What Neoism is

- **Terminal-first**: real PTYs, GPU-rendered text, smooth scrollback, tabs, splits, and command navigation.
- **An editor workspace**: managed Neovim with a Rust-owned file tree, buffer tabs, diagnostics, finder, and workspace chrome.
- **A place for project knowledge**: Markdown, Neoism Notes, Mermaid, notebooks, EPUBs, and `.neodraw` sketches live beside the code.
- **Agent-native**: persistent local agent sessions, parallel sub-agents, LSP, shell and file tools, permissions, checkpoints, undo trees, and durable memory.
- **Multiplayer and remote**: a workspace daemon owns PTYs and shared state so the same workspace can be used from desktop, web, phone, or another laptop over Tailscale.
- **Local-first**: your files, terminals, notes, agents, and credentials stay on machines you control.

Neoism combines a native desktop app, a shared Rust UI, a Rust/WASM web renderer, a workspace daemon, and a standalone agent server. The terminal remains the center while every other surface participates in the same workspace.

## Install

### Linux and macOS

```sh
curl -fsSL https://raw.githubusercontent.com/parkers0405/neoism/main/scripts/install.sh | bash
```

### Windows

```powershell
irm https://raw.githubusercontent.com/parkers0405/neoism/main/install.ps1 | iex
```

Prebuilt releases install `neoism`, `neoism-workspace-daemon`, and `neoism-agent`. Neoism expects `nvim` and `ripgrep` on `PATH`.

Update an existing installation with:

```sh
neoism update
```

Build the full stack from source with:

```sh
git clone https://github.com/parkers0405/neoism.git
cd neoism
./install.sh
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

## Development

Run an isolated development instance without touching your installed Neoism state:

```sh
make dev-isolated
```

Useful checks:

```sh
cargo check --workspace
cd neoism-frontend/web && npm run typecheck
```

Neoism is open source under the [MIT License](LICENSE). Its terminal core and GPU renderer descend from [Rio](https://github.com/raphamorim/rio), which descends from Alacritty. The editor, agent runtime, workspace daemon, sync layer, notebooks, drawings, and Neoism UI are first-party. See [NOTICE](NOTICE) for attribution.