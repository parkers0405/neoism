# Neoism

<!-- Sample edit: testing agent file changes without changing visible documentation. -->

A native workspace for editing code, writing Markdown, running terminals, and working with agents.

Neoism brings editing, shells, and Agent conversations into one window, with persistent workspaces and daemon-owned sessions that keep running when you close the frontend. The GPUIX frontend combines SolidJS with native GPUI rendering, shared Rust editor engines, and Ghostty terminal emulation.

## Features

- Code editing with Tree-sitter, Vim editing, multicarets, save/recovery, and language-server completion, diagnostics, navigation, and refactoring.
- Rich Markdown with source editing, links, tasks, tables, diagrams, and shared documents.
- Ghostty terminals with scrollback, selection, search, and Vim navigation.
- Agent conversations with streaming, model selection, reasoning, attachments, questions, permissions, tools, and history.
- Workspace and buffer tabs, splits, file tree, Finder, command palette, settings, themes, SSH connections, and collaboration.

Start with Alt+E for files, Alt+A for a new Agent, Alt+T for a terminal, Ctrl+P for Finder, Alt+P for commands, and Ctrl+S to save. See the [frontend guide](neoism-frontend/gpuix/README.md) for detailed behavior, shortcuts, and known limitations.

## Install

Download a native bundle from [Releases](https://github.com/parkers0405/neoism/releases). Release targets are Linux x86_64, macOS Apple Silicon, and Windows x86_64.

| Platform | Installation |
|---|---|
| Linux x86_64 | Extract `neoism-linux-x86_64.tar.gz`, then run `./neoism` from the extracted directory. To install into `~/.local/bin`, run `bash install-bundle.sh . "$HOME/.local"` there. |
| macOS Apple Silicon | Open the `neoism-darwin-aarch64.dmg` disk image and drag `Neoism.app` into Applications. |
| Windows x86_64 | Run `Neoism-x86_64.msi`. A portable `neoism-windows-x86_64.zip` is also available. |

Keep the complete extracted bundle together: executables, native libraries, runtime resources, and licenses are required. Distribution is native-only; no browser GUI is automatically built, bundled, or served. The bundle's `web/app` and `web/native` directories hold native runtime resources, not a web application.

### Nightly and updates

Nightly builds for all three platforms are published as prereleases on the [Releases page](https://github.com/parkers0405/neoism/releases). Choose the latest nightly there for a fresh installation, or update an installed copy:

```sh
neoism update --nightly
```

Use `neoism update` for the stable channel. Windows updates use the MSI installer. Linux upgrades retain the previous installed bundle while its windows or daemons are running; a later upgrade prunes it after those processes exit.

## Configuration and connections

Configuration is JSONC in `~/.config/neoism/config.json`, or under `NEOISM_CONFIG_HOME`. Launch options include `--workspace ID`, `--config PATH`, `--theme NAME`, `--font-size SIZE`, and `--maximized`.

A normal launch reuses the local workspace daemon or starts one when needed. To attach to another host, use `neoism --daemon ws://HOST:7878/session`; explicit endpoints never start a local daemon. The daemon owns PTYs, files, language servers, Git, search, collaboration, and Agent sessions.

## Build and develop

The root Makefile provides the local Linux workflow. Install Node.js/npm, a C/C++ toolchain, pkg-config, Fontconfig/FreeType, GBM, xkbcommon, Wayland/X11 development libraries, and the Vulkan loader. Scripts install project-local Bun and fetch the pinned Blaze, GPUIX, and Ghostty toolchains as needed.

```sh
make build                       # build services and the native bundle
./neoism-frontend/gpuix/dist/neoism
make run                         # build services/core, then run from source
make dev                         # source frontend with hot replacement
make check                       # types, design, Rust, and dependency checks
make test                        # workspace, bridge, and frontend tests
```

For a Linux source installation, `./install.sh` builds and installs into `~/.local/lib/neoism-gpuix/bin`, leaving `~/.local/bin/neoism` and existing desktop shortcuts untouched. Launch it with `~/.local/lib/neoism-gpuix/bin/neoism`. To install at the usual user prefix instead, use `./install.sh --prefix "$HOME/.local"`; this replaces that launcher's bundle.

Local Makefile builds and tests use `scripts/blaze.sh`; the native bridge defaults to Blaze but supports an explicitly selected Cargo build. Published release CI uses Cargo for both services and the bridge on Linux, macOS, and Windows through [`scripts/build-gpuix-release.mjs`](scripts/build-gpuix-release.mjs), with platform packaging in [`release-neoism.yml`](.github/workflows/release-neoism.yml). These are distinct workflows, not automatic compiler fallbacks. The [frontend guide](neoism-frontend/gpuix/README.md#run) documents bridge selection, build profiles, native-window checks, and performance measurement.

Nix users can run a built Linux bundle with `nix run .#neoism-runtime -- /path/to/neoism`. The Nix source package provides daemon/Agent services; build the desktop bundle through `make build`. Building those services with Blaze uses `nix build .#neoism-services --option sandbox relaxed` and requires a trusted Nix user; `nix develop` runs the pinned compiler outside that filesystem sandbox.

## Architecture and extensions

| Path | Responsibility |
|---|---|
| [`neoism-frontend/gpuix`](neoism-frontend/gpuix/README.md) | Native GPUIX application, window, input, composition, and Ghostty adapter |
| `neoism-frontend/shared` | Shared editor, Markdown, panel state, layout, interaction, and paint interfaces |
| `neoism-paint` | CPU fonts, shaping, glyph rasters, image data, and retained document layout |
| `neoism-workspace-daemon` | Workspace services, PTYs, transport, persistence, and collaboration |
| [`neoism-agent`](neoism-agent/README.md) | Agent server, CLI, plugins, SDK, and tools |
| `neoism-backend` | Shared persistent configuration and server registry |

Extend Neoism through native/process Agent plugins, MCP, and language servers. See [plugin authoring](docs/plugins.md), the [Agent API reference](docs/agent-plugins.md), and the [Agent runtime](neoism-agent/README.md). Standalone Agent browser tools are explicit opt-ins; normal listeners are API-only, and daemon static UI hosting requires explicit configuration.

Neoism is MIT-licensed; see [LICENSE](LICENSE) and [NOTICE](NOTICE) for licensing and third-party acknowledgements.
