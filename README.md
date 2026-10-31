# Neoism

A native workspace for code, Markdown, terminals, and agents.

GPUIX is the frontend built by this branch. Its preview installation stays separate from an existing daily Neoism launcher. SolidJS drives the native GPUI window, while a Rust bridge provides editor behavior, text shaping, document layout, and retained paint commands. Ghostty supplies terminal emulation. The workspace daemon owns PTYs, files, language servers, Git, search, collaboration, persistent workspaces, and Agent sessions.

The old winit/Sugarloaf desktop, Rio terminal emulator, WASM workspace frontend, and Lua configuration/plugin runtimes are retired. Useful UI and editor code remains in `neoism-frontend/shared`; reusable CPU font and layout routines live in `neoism-paint`. Neither crate creates a GPU device or OS window.

## Build and run

The validated desktop target is Linux x86_64. Install Node.js/npm, a C/C++ toolchain, pkg-config, Fontconfig/FreeType, GBM, xkbcommon, Wayland/X11 development libraries, and the Vulkan loader. Build scripts fetch the checksum-pinned Blaze compiler and its toolchain, plus the pinned GPUIX and Ghostty engines when needed. Rust builds and tests use `scripts/blaze.sh`; there is no application fallback to Cargo.

```sh
make build
./neoism-frontend/gpuix/dist/neoism
```

The bundle contains `neoism`, `neoism-workspace-daemon`, `neoism-agent`, two native libraries, and the standalone Agent web GUI. Keep them together. A normal launch reuses the local daemon or starts one when missing. Closing the window leaves daemon-owned shells and sessions running.

```sh
make run                         # build the engines, then run from source
make dev                         # source frontend with hot replacement
bash scripts/blaze.sh run -p neoism -- --help  # frontend development entrypoint
./install.sh                     # install the separate GPUIX preview
~/.local/lib/neoism-gpuix/bin/neoism  # launch that preview
```

The installer defaults to `~/.local/lib/neoism-gpuix`; it preserves the existing `~/.local/bin/neoism` launcher and desktop shortcuts. An explicit `--prefix ~/.local` replaces that launcher. For an isolated development instance, use `bash scripts/dev-neoism.sh dev`.

Upgrades retain the previous installed bundle while its windows or daemons are
running, so their helpers and resources remain available. A later upgrade prunes
that bundle after those processes exit.

For packaged releases, update to the nightly with `neoism update --nightly`.
Use this spelling when upgrading an older Sugarloaf installation too. The new
GPUIX release additionally accepts `neoism --update nightly`; older executables
may interpret `nightly` as a filename instead. Windows updates install the MSI.
Each platform becomes available after its own release checks pass.

Use `neoism --daemon ws://HOST:7878/session` to attach explicitly to another host. Explicit endpoints do not start a local daemon. `--workspace ID`, `--config PATH`, `--theme NAME`, `--font-size SIZE`, and `--maximized` select launch preferences. Configuration is JSONC in `~/.config/neoism/config.json` or `NEOISM_CONFIG_HOME`; `init.lua` is no longer evaluated.

Terminals load the real pinned `libghostty-vt` library for terminal emulation, keyboard/mouse encoding, selection, and search. GPUIX paints its cell data; the daemon owns the shell and PTY. Fonts, colors, and supported terminal preferences come from Neoism's configuration. The standalone Ghostty application's configuration is not loaded.

## Working surfaces

- Code editing with Tree-sitter, Vim editing, multicarets, save/recovery, and daemon-backed LSP.
- Rich Markdown with source editing, links, tasks, tables, diagrams, and shared documents.
- Ghostty terminals with direct daemon PTY input, scrollback, selection, search, and Vim navigation.
- Agent conversations with streaming, models, reasoning, attachments, questions, permissions, tools, and history.
- Workspace and buffer tabs, splits, file tree, Finder, command palette, settings, themes, SSH systems, and hosted connections.

Shortcuts include Alt+E for files, Alt+A for a new Agent, Alt+T for a terminal, Ctrl+P for Finder, Alt+P for commands, and Ctrl+S for save.

[The frontend guide](neoism-frontend/gpuix/README.md) records working behavior, known parity gaps, native checks, and performance measurement. Native editor IME and notebook/EPUB surfaces remain separate frontend work; retiring the old renderer does not establish feature or FPS parity.

## Agent and extensions

Native Agent plugins, supervised process plugins, MCP, LSP servers, and managed extension installation remain daemon features. The bundled Lua runner and editor Lua plugin host are removed. See [plugin authoring](docs/plugins.md), [the Agent API reference](docs/agent-plugins.md), and [the Agent runtime](neoism-agent/README.md).

The standalone Agent GUI is served at `/agent-gui/`; it is independent of the retired browser workspace renderer.

## Validation

```sh
make check
make test
bash scripts/check-frontend-deps.sh
bash scripts/package-gpuix.sh
python3 scripts/test-gpuix-release.py  # isolated installed release and native-window checks
```

The dependency guard checks both resolved frontend and daemon graphs for retired renderers, window hosts, terminal emulators, and Lua libraries. Native window checks are described in the frontend guide; they require a graphics session.

## Architecture

| Path | Responsibility |
|---|---|
| `neoism-frontend/gpuix` | Native application, GPUIX window, input, composition, and Ghostty adapter |
| `neoism-frontend/launcher` | Rust development launcher for GPUIX |
| `neoism-frontend/shared` | Portable editor, Markdown, panel state, layout, interaction, and paint interfaces |
| `neoism-paint` | CPU fonts, shaping, glyph rasters, image data, and retained document layout |
| `neoism-customization` | Styling and retained contribution data; no scripting runtime |
| `neoism-terminal-types` | Colors, coordinates, input modes, and presentation data; no terminal emulator |
| `neoism-workspace-daemon` | Workspace services, PTYs, transport, persistence, and collaboration |
| `neoism-agent` | Agent server, CLI, native/process plugins, SDK, tools, and standalone web GUI |
| `neoism-backend` | Shared persistent configuration and server registry |
| `neoism-terminal-pty` | Native PTY ownership used by the daemon |

Nix users can run a built Linux bundle with `nix run .#neoism-runtime -- /path/to/neoism`. The Nix source package provides daemon/Agent services; build the desktop bundle through `make build`.

Source-building the Nix services with Blaze uses `nix build .#neoism-services --option sandbox relaxed` (a trusted Nix user is required). Blaze rejects the unmapped owner of Nix's filesystem-sandbox root, so this derivation opts out of that sandbox while keeping pinned inputs and offline dependencies. `nix develop` runs the same pinned compiler outside that filesystem sandbox. The runtime-only package needs no sandbox override.
