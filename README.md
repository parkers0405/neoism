# Neoism

<!-- Sample edit: testing agent file changes without changing visible documentation. -->

**Your terminal. Your IDE. Your agent platform.**

Neoism is a local-first, open-source agentic IDE. A modern terminal, a full code editor, and Neoism's own agent runtime and SDK come together in one persistent workspace.

Run commands, delegate coding tasks, and make changes yourself. Move between shell output, source files, and agent conversations while staying in your project. Build your own agent-powered applications and tools on the same framework that powers the IDE.

**MIT licensed. Native desktop applications for Linux, macOS, and Windows.**

[Download](https://github.com/parkers0405/neoism/releases) · [Get started](docs/README.md) · [Agent SDK](neoism-agent/sdk/typescript/README.md) · [Art](https://neoism.ai/art)

## Build with agents. Stay in control.

- **A command line you can work with.** Ghostty-powered terminals organize commands and output into blocks, with completion previews, command history, filtering, copying, and reusable commands. Choose the block interface or the classic terminal.
- **A full IDE alongside your shell.** Edit with Tree-sitter, Vim motions, multicarets, and language-server completion, diagnostics, navigation, rename, code actions, and formatting. Keep source files, Git changes, and rich Markdown close at hand.
- **Agents that work with your project.** Give agents tasks, follow their progress, inspect tool calls and changes, and respond to permission requests. Choose your model provider and extend their tools through plugins and MCP.
- **An agent framework you can build on.** Neoism includes its own embeddable Rust agent runtime, TypeScript SDK, and plugin framework. Build applications and integrations around sessions, streaming events, tools, permissions, and provider connections.
- **A workspace that stays with you.** Arrange files, terminals, and agents in tabs and splits. Your local workspace daemon keeps shell and agent sessions running independently of the frontend, with remote connections and collaboration available when you need them.

Looking for a **Warp alternative**? Neoism combines a block-based terminal with a full IDE and a built-in agent development platform, in a local-first, MIT-licensed application.

Your application, workspace services, and agent runtime run locally by default. Model inference happens with your configured provider; connected providers, MCP servers, and remote workspaces have their own data boundaries. See [what local-first means](docs/README.md#local-first-by-default).

## Install

Download a native bundle from [Releases](https://github.com/parkers0405/neoism/releases). Release targets are Linux x86_64, macOS Apple Silicon, and Windows x86_64.

| Platform | Installation |
|---|---|
| Linux x86_64 | Extract `neoism-linux-x86_64.tar.gz`, then run `./neoism` from the extracted directory. To install into `~/.local/bin`, run `bash install-bundle.sh . "$HOME/.local"` there. |
| macOS Apple Silicon | Open the `neoism-darwin-aarch64.dmg` disk image and drag `Neoism.app` into Applications. |
| Windows x86_64 | Run `Neoism-x86_64.msi`. A portable `neoism-windows-x86_64.zip` is also available. |

Keep the complete extracted bundle together: executables, native libraries, runtime resources, and licenses are required. The bundle's `web/app` and `web/native` directories contain resources for the native desktop application. See the [packaging notes](neoism-frontend/gpuix/README.md#run) for distribution details.

### Nightly and updates

Nightly builds for all three platforms are published as prereleases on the [Releases page](https://github.com/parkers0405/neoism/releases). Choose the latest nightly there for a fresh installation, or update an installed copy:

```sh
neoism update --nightly
```

Use `neoism update` for the stable channel. Windows updates use the MSI installer. Linux upgrades retain the previous installed bundle while its windows or daemons are running; a later upgrade prunes it after those processes exit.

## Make something

Open your project, press Alt+T for a terminal, and run your first command. Alt+E opens files, Alt+A starts an Agent conversation, Ctrl+P opens Finder, Alt+P opens commands, and Ctrl+S saves your work.

Open **Documentation** from the command palette, or run `neoism docs`, for the local handbook and Scalar API reference. The same documentation is [available online](https://neoism.ai/api/docs/). New Notes vaults start with one root-level `Welcome.md` checklist covering the basics; the full handbook stays out of your notes.

Start with [documentation access](docs/README.md), explore the [block terminal](docs/terminal.md), or build with the [Agent SDK](neoism-agent/sdk/typescript/README.md). The [frontend reference](neoism-frontend/gpuix/README.md) covers detailed behavior, shortcuts, and current limitations.

## Software is art, too

**Art should be free to enjoy. Software should be, too.**

You can explore Michelangelo's *The Last Judgment / Christ the Judge detail* in [Neoism Art](https://neoism.ai/art). The gallery offers a public-domain reproduction to look closely at, learn from, and enjoy. We bring that same spirit to the tools we build.

Software is a creative work. You should be able to use it, understand how it works, change it, and share what you make. Neoism is free and open source under the [MIT license](LICENSE): the freedom to make the workspace your own is part of the product.

## Configuration and connections

Configuration is JSONC in `~/.config/neoism/config.json`, or under `NEOISM_CONFIG_HOME`. Launch options include `--workspace ID`, `--config PATH`, `--theme NAME`, `--font-size SIZE`, and `--maximized`.

A normal launch reuses the local workspace daemon or starts one when needed. To attach to another host, use `neoism --daemon ws://HOST:7878/session`; explicit endpoints never start a local daemon. The daemon owns PTYs, files, language servers, Git, search, collaboration, and Agent sessions.

## Build and develop

The root Makefile provides the local Linux workflow. Install Node.js/npm, a C/C++ toolchain, pkg-config, Fontconfig/FreeType, GBM, xkbcommon, Wayland/X11 development libraries, and the Vulkan loader. Scripts install project-local Bun and fetch the pinned Blaze, GPUIX, and Ghostty toolchains as needed.

Clone with `git clone --recurse-submodules https://github.com/parkers0405/neoism.git`, or initialize the pinned shared docs in an existing checkout with `make docs-init`. `make docs` builds the local handbook and API site without compiling the application. Documentation source is maintained in [neoism-docs](https://github.com/parkers0405/neoism-docs), shared with the website through pinned Git submodules.

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

The native frontend combines SolidJS with GPUIX/GPUI rendering, shared Rust editor engines, and Ghostty terminal emulation. Workspace services and the embeddable agent runtime live behind the desktop interface.

| Path | Responsibility |
|---|---|
| [`neoism-frontend/gpuix`](neoism-frontend/gpuix/README.md) | Native GPUIX application, window, input, composition, and Ghostty adapter |
| `neoism-frontend/shared` | Shared editor, Markdown, panel state, layout, interaction, and paint interfaces |
| `neoism-paint` | CPU fonts, shaping, glyph rasters, image data, and retained document layout |
| `neoism-workspace-daemon` | Workspace services, PTYs, transport, persistence, and collaboration |
| [`neoism-agent`](neoism-agent/README.md) | Embeddable agent runtime, server launcher, plugins, SDK, and tools |
| `neoism-backend` | Shared persistent configuration and server registry |

Build on the [TypeScript SDK](neoism-agent/sdk/typescript/README.md), extend the runtime with [native/process Agent plugins](docs/plugins.md), or connect tools through MCP. See the [Agent API reference](docs/agent-plugins.md) and [Agent runtime guide](neoism-agent/README.md) for contracts, permissions, and deployment options.

Neoism is MIT-licensed; see [LICENSE](LICENSE) and [NOTICE](NOTICE) for licensing and third-party acknowledgements.
