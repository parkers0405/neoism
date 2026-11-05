# Neoism GPUIX frontend

The native desktop frontend for Neoism, a local-first, open-source agentic IDE. It brings the block terminal, full code editor, rich Markdown, and Neoism's own agent platform into one persistent workspace. Start with the [workspace guide](../../docs/README.md) or [terminal guide](../../docs/terminal.md) for everyday use; this page covers frontend development, implementation details, and compatibility.

SolidJS renders through GPUIX/GPUI; Effect manages the daemon connection and typed requests, and the Neoism TypeScript SDK handles Agent APIs. Published releases target Linux x86_64, macOS Apple Silicon, and Windows x86_64; see the [main README](../../README.md#install) for installation and nightly updates.

## Run

With the existing Neoism daemon running, from this directory:

```sh
npm ci
npm run setup:runtime
npm run build:core
npm start
```

Blaze is the default Rust builder for the local Makefile workflow, native bridge, workspace test runner, and Nix services package. Repository-root `scripts/blaze.sh` and `scripts/setup-blaze.mjs` install the exact published release and verify its SHA-256 against this frontend's `native/blaze-pin.json`.

The root Makefile workflow is Linux-oriented: `make build` builds the services, native bridge, and native application bundle; `make run` builds services/core and runs from source, and `make dev` enables hot replacement. Root `./install.sh` installs the Linux bundle in `~/.local/lib/neoism-gpuix/bin`, preserving `~/.local/bin/neoism` and existing desktop shortcuts. An explicit `--prefix "$HOME/.local"` installs at the usual user prefix instead.

Application builds never automatically fall back to another compiler. For an explicitly selected optimized native bridge, use `NEOISM_GPUIX_RUST_BUILDER=cargo NEOISM_GPUIX_BUILD_PROFILE=release npm run build:core`, then `npm run build -- --reuse-core` to package that verified library. This selects the bridge compiler only; root Makefile service builds still use Blaze. The bridge's Cargo output is isolated in `target/gpuix-core-cargo`.

Published release CI uses Cargo for services, native bridge, and updater tests on all three platforms, through repository-root `node scripts/build-gpuix-release.mjs`. The script explicitly selects the Cargo bridge builder and release profile, validates and tests the native bundle, and invokes platform packaging. The release workflow additionally builds and validates the Windows MSI; Windows release CI does not bootstrap or test with Blaze.

Set `NEOISM_GPUIX_DISPLAY_SOURCE` to a separate checkout directory when building the pinned display renderer alongside local renderer edits. The build still verifies the revision, patches, and source hashes; it never resets a conflicting checkout.

Native local and CI releases neither build nor package browser UIs. The `web/app` directory contains the native application's JavaScript bytecode/assets, and `web/native` contains native libraries; both remain required. `web/index.html` and `web/agent-gui/index.html` are comment-only compatibility markers required by already-installed old updaters, not browser entrypoints. Staging removes stale browser assets before recreating those markers; `web/agent-gui` must contain only its inert `index.html`, with no scripts, redirects, or assets. Normal Agent listeners are API-only; standalone Agent `gui`/`web` commands and `npm --prefix neoism-agent/sdk/typescript run gui:build` from the repository root remain explicit opt-ins. The workspace daemon serves static UIs only when explicitly configured with `NEOISM_WEB_ROOT` or `NEOISM_AGENT_GUI_ROOT`; it does not discover installed or source-checkout browser assets. REST/WebSocket routes and authentication are unchanged.

After building and testing the native bridge, `npm run build -- --reuse-core` packages that existing library after another FFI smoke check. The normal build compiles the bridge through the selected compiler (Blaze locally by default, Cargo in release CI) before packaging.

For local Windows Blaze builds, upstream's documented `BLAZE_PROC_MACRO_SERVER=0` host macro mode is used because its matching native macro server is unpublished. The compiler's [documented native macro host](https://github.com/indexable-inc/blaze/blob/8f144c2dae639fd093e4c489dace50132002fe0a/docs/cargo.md) handles SQLx and `enum-as-inner`.

`npm run benchmark:render` measures uncached sidebar rasterization, Markdown
opening, and repeated Markdown hover rendering without opening a window or
connecting to a daemon. Set `NEOISM_GPUIX_CORE` to compare native libraries in
separate processes. Compare the output hashes as well as timings, and use the
same source revision and machine for build comparisons. These are native CPU
workloads, not measurements of application FPS or total startup time.
`npm run benchmark:cursor` compares full editor frames with cursor-only updates
against the same native library and viewport. Cursor-only updates retain the
document rows; deferred keyboard input, scrolling, and LSP work still request
full frames. The component regression also checks that cursor animation does
not rewrite the document canvas or upload its glyphs.
Fractional editor scroll steps translate retained row ink instead of resending
all glyph commands. The same component regression checks 80 sub-row offsets,
native row alignment, edge-row replacement, and edits. Wheel notches convert
to logical cell pixels; monitor scale is applied by GPUI, not a second time by
the input handler.
Terminal text, backgrounds and underlines share a single canvas instead of a
layout node per styled run. `tests/terminal-paint.test.ts` covers dense ANSI
scrollback, cell geometry, fractional edge rows, and selection in the actual
component without creating a window.

Build profiles select Blaze's backend: development uses Cranelift and release
uses LLVM at `-O3`. The wrapper preserves an explicit `BLAZE_BACKEND` override.

`node scripts/test-workspace.mjs --locked` from the repository root tests every
workspace member with Blaze, including its integration tests and documentation
examples. The pinned compiler fails this repository's combined `--workspace`
test graph with `unsupported: no lang_start`. The runner selects each unit and
integration target separately, builds the examples, and runs documentation tests.
This also bounds retained compiler state that otherwise grows across a package's
combined targets. It continues after failures and fails if any member fails.
Development tests use Cranelift consistently and omit debug information to bound
compiler memory, while retaining debug assertions and overflow checks.
Explicit profiles and `BLAZE_BACKEND` overrides are preserved; the optimized
application and native release tests use LLVM at `-O3`.
If the compiler reports incompatible cached function signatures before tests
start, the workspace runner retries that target once with a fresh compiler home,
target directory, and disabled cache. This handles ICU's feature-dependent vector
layout in the pinned compiler's shared store. Test failures are never retried.
Before documentation tests, the runner closes the completed unit/integration
compiler. Blaze runs doctests in a separate process; keeping both large databases
resident causes swapping. Their compiled functions remain in the disk store.

The [dependency compatibility record](../../vendor/blaze-compat/README.md)
documents the pinned compiler's locale-data, allocator-method, and AES support
workarounds, along with the unchanged dependency versions and licenses.
The wrapper defaults to an 8 GiB budget for retained workers and disables
speculative workers. `BLAZE_MEMORY_MAX` and `BLAZE_SPARE` can override those
defaults; a currently compiling worker is retained even if it exceeds the budget.
`BLAZE_CACHE_MAX` controls the separate disk-store limit, which otherwise depends
on free disk space. For large local workspace runs, `BLAZE_CACHE_MAX=32G` retains
more compiled functions when space permits. An explicit `BLAZE_REMOTE` is passed
through to Blaze; remote caching stays off when none is configured.
Full builds close their private service compiler session before loading the
native frontend, while sharing the downloaded toolchain and persistent cache.
On Linux the wrapper limits compiler pools to four permitted CPUs; Cargo's
jobserver does not limit Blaze's whole-program pools. `NEOISM_BLAZE_CPUS` changes
this limit (`0` removes it). Run `bash scripts/blaze.sh stop` before changing the
limit so the next compiler daemon inherits the new affinity.
After services have built successfully, `bash scripts/gpuix.sh build-frontend`
rebuilds the native bridge and frontend bundle using those service executables.
The native bridge enables mimalloc's `local_dynamic_tls` mode because Bun loads
it into an already running process; reserving static TLS at application startup
is not required.

`npm run dev` enables hot replacement. npm scripts select project-local Bun
1.3.13, which supports Unix WebSockets. `setup:runtime` also works when npm blocks
Bun's install script. Native libraries are replaced atomically.

Packaged builds use the selected compiler's release profile. Standalone `build:core` defaults to the development profile; set `NEOISM_GPUIX_BUILD_PROFILE=release` for the packaged profile. Blaze bridge outputs use `target/gpuix-core-blaze`, and Cargo bridge outputs use `target/gpuix-core-cargo`, independently of the services' build directory. `NEOISM_GPUIX_TARGET_DIR` overrides that directory.

The app reads `~/.config/neoism/config.json` (JSONC), discovers the local daemon socket and token, or connects to `ws://127.0.0.1:7878/session`. It reuses a daemon only after a successful application handshake and automatically starts/retries a missing local service. An explicit `--daemon` endpoint never starts a local service.

Automatically managed shared services prefer daemon port 7878 and Agent port 4096. If an unrelated listener occupies a preferred port, the service reserves a free loopback port and publishes its actual address beside the local daemon token as `daemon-endpoint.json` or `agent-endpoint.json`; discovery is not itself a readiness signal. OS-held ownership locks prevent competing managed owners and release on process exit. Explicit daemon addresses/sockets, explicit Agent URLs, remote connections, and private/Fast previews do not silently change ports, and unrelated processes are never killed to reclaim a port.

```sh
npm start -- --workspace WORKSPACE_ID
npm start -- --daemon ws://HOST:7878/session
npm start -- --help
```

`NEOISM_DAEMON_TOKEN` is the upgrade credential; `NEOISM_PAIRING_TOKEN` is the Hello
credential. Local credential discovery applies only to the default endpoint.

Discord Rich Presence connects to the local Discord desktop client automatically and reconnects if Discord starts later or restarts. It shows only "Building in Neoism", a generic workspace description, and elapsed time; workspace names, paths, and file contents are never sent. Set `NEOISM_DISCORD_PRESENCE=0` to disable it. `NEOISM_DISCORD_APP_ID` and `NEOISM_DISCORD_ASSET_KEY` retain the original desktop integration's optional overrides. Linux IPC discovery includes native, Flatpak, and Snap locations; macOS uses runtime/temp sockets and Windows uses Discord's named pipes. Presence closes with the native window and is not run by the daemon.

## Working paths

- **Open Documentation** in the command palette opens the bundled handbook and Scalar API reference in the browser, served on a loopback-only ephemeral port. `neoism docs` serves the same site without starting graphics or the daemon; Ctrl+C stops its foreground server. Assets live in `resources/docs` and work offline. New Notes vaults receive only a root-level `Welcome.md` checklist, with earlier handbook copies preserved. See [documentation access](../../docs/README.md) and [the shared-source architecture](../../docs/documentation-notebooks.md).

- Databases is a native workspace sidebar alongside Files, Notes, and Conversations, toggled by its topbar database button. Open a SQLite file (`.db`, `.sqlite`, `.sqlite3`, `.db3`) from Explorer or Finder to register it in that workspace's sidebar. The schema tree lists tables, views, columns, indexes, and triggers; selecting a table opens or focuses an ordinary Neoism buffer tab, with its own paged grid, filters, sorting, and draft edits. There is no nested app or second tab system. Double-click a cell to edit it. New rows, edits, and deletions are drafts until Apply; Discard restores the loaded page. NULL, empty text, and omitted/default insert values remain distinct. Views and rows without a safe identity are read-only. Database requests execute on the workspace daemon, including for remote workspaces. This first slice is SQLite-only, not a network connection manager or SQL console.
- Shared Rust CodeBuffer, Vim resolver, syntax, wrap/hit testing, viewport springs,
  and TrailCursor drive the code editor. Independent split views preserve their
  own caret and viewport. Unsaved documents and layout survive window restarts.
- Shared SessionTree drives tabs, splits, directional focus, divider dragging,
  and tab adoption. Splitting creates a fresh terminal, matching the desktop.
- Original Finder, command palette, file tree, and LSP UI state feed
  GPUIX views. LSP includes completion, hover, diagnostics, navigation, rename,
  code actions, and formatting. Supported command actions and custom bindings
  dispatch through the app.
- The original Rust top bar, hamburger catalog, status-line pills, Settings,
  appearance pickers, About, and Extensions painters run through a portable
  paint interface. Workspace and buffer tabs use the original Rust chrome
  specifications and geometry. No transport label is added to the status line.
- Markdown opens in the original rich MarkdownPane, retaining its source maps,
  Vim/history, tasks, tables, callouts, code cards, links, search, and diagrams.
  Precise trackpad deltas follow the same direction as the code editor; the
  frontend does not override the compositor's natural-scroll preference.
  Canonical editor buffers still own saves/recovery; each rich view preserves
  its own cursor and scrolling. Covers/ink use the daemon asset boundary.
- Agent history, streaming, model/agent/reasoning pickers, attachments, questions,
  permissions, queue, expanded tools/diffs, and virtualized timeline are connected
  to the daemon. Empty Agent sessions use the original home input/wordmark
  geometry; the original Rust footer paints checkout, command/connect hints,
  and activity. Slash commands merge the canonical catalog with SDK commands
  and execute real session/provider actions. Child sessions hide the composer.
- Home integrates the splash with a block terminal by default. The `+` after the buffer tabs opens another Home tab. With Block terminal disabled, Home is a standalone splash and the first explicitly opened Terminal owns local cwd navigation; hidden Home shells do not take that role. Recovered dirty documents stay open while Home is selected. Wordmark click effects use a persistent per-surface shuffle bag; automatic playback follows `appearance.splash-auto-animate` (default false).
- libghostty is the terminal engine for both block and classic presentation. The block terminal adds an editable command bar, completion and history previews, and selectable command/output blocks. Full-screen applications use the native grid. Hidden tabs continue parsing output, and closing the frontend leaves daemon shells alive.
- Configured palettes, fonts, wallpaper/cached blur, original dimensions, and
  animation state are reused. Native clipboard operations use platform tools.

GPUIX owns GPU painting, compositing, and the OS window. CPU font shaping, measurement, and glyph rasterization live in `neoism-paint`; cached pixels become GPUI image textures. Shared UI entrypoints use the portable `Paint` interface. `neoism-customization` holds styling and contribution data.

Shortcuts include Alt+E/C/N/H, Alt+S or Ctrl+P for Finder, Alt+P or Ctrl+Shift+P for commands,
Alt+T or Ctrl+T for a terminal, Ctrl+S for save, and shared Vim motions/operators/registers/
macros. Terminal Ctrl+Shift+C/V uses the system clipboard; Alt+Shift+Space toggles
terminal Vim navigation. Editor multicaret operations use Ctrl+Alt+arrows and
Ctrl+D (standard/Insert) or `gb` (Vim).
Alt+A opens a fresh Agent tab with its own draft each time. In Normal mode,
Space then X closes the current code or Markdown tab. In Alt+P, `q`/`:q` close
only the current tab; `qall`/`:qall` close the current workspace. Unsaved files
require saving or explicit `!` discard.
Ctrl/Cmd+Plus/Minus persist the global font size across pages and workspaces.
Scale-dependent views swap in one native mutation batch under the same window
root, retaining the model, drafts, and editor buffers. The wallpaper, wordmark
textures, tree scroll hosts, and completed reveal clocks remain across zoom;
the splash and tree glyphs paint immediately from their retained dimensions.
Opening the file tree also seeds fresh row widths before the first layout event,
so its content appears immediately with one 120 ms group slide. Rows have no
invisible stagger. Uncached directory listings show a visible skeleton immediately.

Workspace tabs sit directly above the buffer tabs, beside the sidebar. Both rows
scroll horizontally and keep their `+` immediately after the last tab. Workspace
tabs fit their titles, icons, and padding; they scroll when the row fills up. The upper
`+` creates a workspace with Welcome; the lower `+` creates a Home buffer.
Both rows support drag reordering with pointer-following feedback and spring-driven
neighbor slides; reduced motion applies the new positions immediately. Reordering
preserves live editor hosts, unsaved buffers and the selected workspace. Workspace
labels are nonselectable during dragging. Restoring a workspace measures its tabs
without replaying entrance animations. Workspace
close buttons and `Ctrl+Shift+W` close and create local window workspaces without
terminating daemon-owned terminals.

Command palettes, Finder and other global dialogs center horizontally across
the whole window (including the sidebar), retain their original upper native
anchor and stay below the top chrome and both tab strips. Composer pickers, LSP
hover and notification banners remain constrained to their focused content body.
LSP hover uses the original measured Rust painter, pointer retention, text selection,
scrolling, links and copy. Agent input uses native shaped caret geometry to paint
the shared block cursor and trail.

Alt+S starts in FILES mode with the original Rust Finder painter. Fuzzy file
searches run in the matching daemon's warm FFF index; exact matching retains the
existing exact collector. An older running daemon must be replaced on the next
normal restart to activate FFF. From the repository root,
`bash scripts/blaze.sh build --profile activation -p neoism-workspace-daemon --bin neoism-workspace-daemon`
uses the existing optimized profile with production service paths and no full LTO.
Building this frontend does not restart it.

The workspace row
restores this frontend window’s explicitly opened workspaces in their saved order;
the daemon’s full catalog remains available through Alt+P → Workspaces. Older
profiles start the row with their selected workspace without deleting any saved
projects, buffers, or terminal sessions.

Plain `h` in Files/Notes toggles dotfiles with the original Rust exclusions;
ArrowLeft still collapses directories. Each workspace, root and panel keeps its
own visibility choice through font generations. Agent composer controls use
bounded original font measurements on narrow panes, truncating model labels
before they can overlap reasoning, attachment or submit controls.

## Servers and multiplayer

The top-right Servers button uses the original Rust Servers/Systems palette and
Host/Join forms. Saved connections support add, edit and remove with masked
credentials. Ordinary Host launches a separate persistent daemon for the chosen
local folder; removing that hosted connection stops that exact daemon recipe.
Fast Host reuses the desktop OpenTunnel implementation and its filtered ingress;
Fast Join accepts complete invite links/passwords without saving temporary access
credentials to the durable server registry. Fast hosts are owned by the app and
stop on exit. Public tunnel provisioning still depends on OpenTunnel availability.

Systems uses the desktop SSH recipe registry, alias parsing, daemon bootstrap and
port-forward owner. The normal Ghostty terminal consumes the selected daemon's PTY
stream: an SSH shell runs on that system, rather than in a local shell pretending
to be remote. Failed/cancelled connections leave the current session intact.
Selected SSH tunnels reconnect with the same recovery identity and tab/workspace
focus. Returning to Local restores its own transport and saved workspace state.
The production package includes the daemon used for ordinary/Fast hosting.

Shared code and rich Markdown use the original Rust Yrs document bindings over
the daemon CRDT plane. Split views share one document owner, native undo is scoped
to the local collaborator, and saves are daemon-owned. Original cursor/selection
flags, top chrome participants and per-file peer avatars use the same presence
identity and workspace scope. Agent tabs subscribe to the same live conversation
with separate local drafts and correct prompt authorship. Integration checks use
two authenticated clients, isolated real daemons, and separate SSH transport
fixtures; they never launch a GPU window or connect to a real SSH host. Shared
document state and pending edits survive recovery; pre-restart Yrs undo stacks
are not serialized. SSH systems need a compatible installed Neoism daemon, as
with the original desktop bootstrap.

## Remaining parity work

This is a working port under development. Complete visual and interaction parity
has not been established. Remaining work includes notebook/EPUB surfaces,
custom-editor IME, remaining desktop management actions,
and equal-sized comparisons of appearance, selection, scrolling,
and animations. Extensions inventory works; installation needs a daemon lifecycle API.
Background catalog/preview currently requires a local host. Command actions that
lack an implementation are hidden from the palette.

Linux x64 is the validated native target. `parity.json` tracks individual gates.

## Experimental device preview

Device preview is an off-by-default **build feature**, not a setting that loads a browser. Published releases explicitly set `NEOISM_GPUIX_DEVICE_PREVIEW=0`, excluding the unfinished iPhone/iPad preview even if the build environment enables it. Build with `NEOISM_GPUIX_DEVICE_PREVIEW=1 npm run build` to include it. The normal `npm run build` excludes the device panel and transport modules, compiles the display binding without the Rust `device-preview` feature, and uses the original artifact identity. Optional builds use a separate source checkout, Cargo target directory, and verified display stamp; they do not replace the ordinary binding.

In an enabled build, click the phone button at the right of the workspace strip or press **Alt+Shift+D**. The right-hand panel reserves layout space rather than covering the editor. **Alt+Right** enters it from the rightmost content pane; **Alt+Left** returns to content. Its open state is per workspace. The Git panel takes precedence when open. Closing the device panel releases its viewport, network streams, and decoder without powering down a simulator.

The native viewport currently supports Linux's GPUI wgpu backend. The model selector covers T3's full four-device lineup using original Neoism geometry, not copied GLBs, CAD, logos, or textures. Models have distinct bodies and displays, camera hardware, edge details, selectable finishes, and studio-style lighting. They are preview representations, not manufacturer CAD or a claim of pixel-identical lighting.

| Model | Preview hardware |
| --- | --- |
| iPhone 18 Pro | Compact Pro body, triple-camera assembly, front glass, buttons and ports |
| iPhone 18 Pro Max | Larger body/display with its own proportions and camera placement |
| iPhone Duo | Articulated halves and hinge, inner display, and outer cover screen |
| iPad Pro 13-inch (M5) | Thin tablet and an optional original keyboard, trackpad, and stand |

The panel follows the supplied screenshot's viewport-first layout: a small model tab, a neutral black (or light-theme neutral) stage, a quiet right-side icon rail, and contextual menus. There is no permanent back-header, status footer, connection form, finish-control row, text-button toolbar, or experimental banner. Click the model title for models/finishes; More contains front/back views, display selection, and connection actions. Device tools opens discovery without reducing preview height. The icon rail provides Home, supported hardware controls, 3D/flat presentation, keyboard attachment where available, and Restore view. UI text uses a sans-serif family instead of the editor font, and the device pane starts at the reference's 540-pixel preferred width when space permits.

Choose a model for an offline preview or let the connected simulator name select its matching profile. The offline preview uses original abstract artwork, not a fabricated OS or a substitute for a failed live stream. Unrecognized devices use a clearly labeled preview override, and manual overrides remain available. The Duo starts in a broad, slightly folded book hold; More can switch between book and portrait holds. The iPad keyboard/stand is landscape. Drag outside the live screen to orbit, Alt-drag to orbit regardless of screen hits, and use the wheel/trackpad to zoom. Consecutive orbit updates preserve the active gesture; changes to physical geometry or input readiness cancel it. Only the focused device screen forwards text, never menus or controls. Connected touches are picked against the native triangles. Flat view contain-fits the raw video.

The output texture is composited directly on GPUI's existing GPU device/queue, without normal-frame GPU-to-CPU readback, Chromium, a second rendering window, or an independent loop. Only video frames and pose changes invalidate the 3D pass. Mesh geometry is reused during orbiting; material and camera changes do not reupload video. Rendering is bounded to a 1536-pixel longest edge to retain HiDPI detail. The optional build also carries a separately pinned Linux window-capture patch for explicit automation screenshots: it re-rasterizes the actual window's last painted client scene using its existing renderer, atlas, and device, then reads it back only on request. Capture does not enable test-support, continuously copy frames, or change the default build. BGRA windows are normalized to RGBA; capture is bounded to 4096 pixels per edge.

Duo hinge controls change the offline model immediately; for a connected device, helper-reported posture remains authoritative. **Inner** and **Cover** select the preview screen and, where supported, request the matching physical orientation from the real helper. Live selection is not an optimistic texture swap: it requires the helper acknowledgement, elected `screenId` (1 = cover, 3 = inner), and fresh video before input resumes. Unsupported helpers disable live display controls. Orbit-driven display following runs when the drag ends and only requests a supported alternate resting face for a partly open hardware pose; failures/timeouts are surfaced rather than pretending a switch succeeded. One elected device stream is active at a time.

Enter an existing trusted **expo-device-hub** URL, click **Connect** to list devices, then explicitly choose **Watch** or **Boot**. Opening the panel or listing devices never starts a decoder, boots a device, downloads helpers, or installs an SDK. An optional `NEOISM_DEVICE_HUB_URL` supplies the initial URL. The URL is not persisted by this panel; use HTTPS for remote connections and treat device streams/input as sensitive. iOS requires a Mac device host with Xcode; Android requires a host with its emulator tooling. The client needs `ffmpeg` on `PATH` (or `NEOISM_DEVICE_FFMPEG` set to an executable path). See [`src/device/HOST.md`](src/device/HOST.md) for the protocol and capability limits.

The initial decoder uses an explicitly launched ffmpeg process and software H.264 decoding, followed by one RGBA upload to the GPU. Decoded output is contain-scaled to a maximum 1280-pixel longest edge, never upscaled, with one filter thread; input coordinates remain normalized to the full device. It keeps only the latest pending decoded frame and stops on backlog/timeouts instead of accumulating input latency. Hardware decoding/zero-copy video import, real-device validation, and integrated-GPU presentation benchmarks remain follow-up work. WebRTC-only hosts and physical-device streams report unsupported errors; there is no screenshot polling or browser fallback.

Hardware actions are capability-gated. iOS screen rotation uses the helper's tagged orientation protocol and waits for confirmed screen metadata; Android user-rotation policy is not presented as a captured-screen rotation API. Appearance uses supported Android helper readback and updates only after confirmation. Save screenshot explicitly requests one PNG from the device helper and writes a uniquely named file under `~/Downloads`; it never screenshots the desktop or becomes a video source. Unsupported REST routes disable the corresponding control without interrupting video. Device screenshots and native-window automation screenshots are separate operations.

```sh
# Validate the optional native source without building a release addon:
NEOISM_GPUIX_DEVICE_PREVIEW=1 bun scripts/build-display-binding.ts --check-only

# Build/package the optional feature through the normal production pipeline:
NEOISM_GPUIX_DEVICE_PREVIEW=1 npm run build

# Return to the ordinary package (the original display artifact stays available):
npm run build

bun test tests/device-preview-build.test.ts tests/device-display-binding.test.ts tests/device-host.test.ts tests/device-panel.test.ts tests/device-interaction.test.ts

# Actual production panel in a focus-disabled native window; PNGs under artifacts/device-preview:
NEOISM_GPUIX_DEVICE_PREVIEW=1 bun scripts/capture-device-panel.ts
```

The transport tests include real H.264/ffmpeg decoding against a local HTTP/WebSocket test hub. The application test opens the actual panel using a recording native boundary, performs discovery and streaming, checks RGBA uploads, continuous orbiting, keyboard isolation, touch forwarding, and cleanup. Native validation separately exercises the 3D renderer. `capture-device-panel.ts` opens the production component in an actual native window and captures the complete client scene, including typography, icon rail, menus, and model, instead of treating a custom-pass render as proof of the panel's appearance. These checks do not substitute for live simulator or integrated-GPU performance validation.

## Block-terminal completion

The block-terminal UI is enabled by default. Turn off **Block terminal** in Settings, or set `"terminal": { "block-ui": false }`, to return to the classic terminal and standalone Home splash. Terminal presentation applies to new sessions; existing running shells are not restarted.

With `terminal.block-ui` enabled, Home and terminal tabs use a session-bound completion service. Tab previews candidates without submitting; Shift+Tab cycles backward, Escape restores the original query, and Enter submits once. Up/Ctrl+R open history, where navigation previews entries directly in the input bar; Enter accepts a history preview without executing it. Typing refreshes completion results while retaining the visible menu, with stale candidates blocked from overwriting the draft. Empty results produce a short input highlight rather than an empty popup. See the [terminal guide](../../docs/terminal.md) for everyday controls.

An updated daemon and a freshly started integrated shell provide host PATH commands, live aliases/functions/variable names, quoted paths, Git branches/remotes/changed files, and package scripts. Bash also uses registered `-F` completion functions, literal `-W` lists, and filename/directory registrations. Small built-in command hints supplement hosts without completion plugins. Native candidates precede history, and requests use the terminal's host-side cwd rather than the Explorer's directory.

Shell compatibility has specific limits: zsh currently supplies explicitly identified live-name and host-data fallback rather than native ZLE/compdef completion. Readline-dependent and `-C` Bash completers, compound shell programs, and command substitutions are not supported. Older daemons and unsupported shells retain the bounded app-side fallback.

Completion uses private prompt-state snapshots and isolated, time/output/concurrency-bounded workers. It never evaluates the query or writes it to the live PTY. Because installed completion functions can run helper programs, requests require terminal-execution permission, attachment, and workspace authorization; file-read permission alone is insufficient. Workers run outside the websocket receive loop and are cancelled on superseding requests, input, close, or disconnect.

The Files panel retains the original workspace reveal: a 320 ms cubic fade/slide with a 12 ms stagger per visible row. Only glyphs move; hit targets and selection backgrounds remain fixed. Cached connection returns replay it, while same-root refreshes and font/theme rebuilds do not. Reduced motion disables it.

## Validate and package

```sh
npm run typecheck
npm run check:design
npm test
npm run smoke:daemon
npm run check:native
npm run build
npm run check:native -- --packaged
```

`bun scripts/native-splash-check.ts --packaged`
verifies the independent default Home tab, all eight source actions, actual
native wordmark canvas, visual-only logo clicks, local Agent drafts, and
workspace return without creating a terminal. The new-tab `+` creates another
Home tab with no PTY. Workspace tabs include a close `×`, and both
workspace and buffer tabs support horizontal drag reordering. Explicit New Terminal opens a separate Grid PTY without
a splash overlay. `bun scripts/native-tab-workflow-check.ts --packaged` also
checks recovery startup, overflowed tabs, closable main terminals, Alt+T across
surfaces and Settings, global zoom/root retention, breadcrumbs, tab caret
alignment, and wordmark paint extending beyond its logo hitbox.
`bun scripts/native-workspace-window-check.ts --packaged` verifies sidebar and
workspace-row alignment, catalog isolation, opening a saved workspace through the
picker, and restoring that window’s tabs across relaunch.
`bun scripts/native-workspace-tabs-check.ts --packaged` covers title-sized scrolling
workspace tabs, inline add buttons, real workspace creation without a PTY, and
keyboard access to overflowed tabs and the workspace add button.

`bun scripts/native-close-commands-check.ts --packaged` checks all tab types,
leader close, independent Agent drafts, workspace close, and buffer drag order.
`bun scripts/native-zoom-stability-check.ts --packaged` checks retained wallpaper
and tree identity, completed reveals, and visible splash painting during rapid zoom.

`bun scripts/native-agent-composer-check.ts` verifies Alt+A home centering,
all canonical slash rows plus SDK commands, real reasoning updates, Enter
selection, the original footer geometry, and chat bottom anchoring.
`bun scripts/native-composer-float-check.ts` captures a real GPUIX window and
checks for transcript pixels outside the rounded input card and inset footer,
including the area below the input. It requires Hyprland, grim, and system Python
with Pillow, like the other native pixel checks.
`bun scripts/native-navigation-check.ts` verifies Alt focus/number navigation,
Finder focus restoration, and numeric/final-line code and Markdown commands.

Additional native checks cover Agent controls, layout, timeline, recovery, LSP,
Finder, commands, original chrome/menus, rich Markdown, scrolling/cursor
focus, and the real daemon terminal. `npm run benchmark:scroll` separately measures
idle bridge frames and paced scroll input, asserting that the visible document
or terminal scrollback actually moves. It opens no windows and excludes Solid,
GPUI layout/paint, texture uploads, and presentation; its timings are not FPS.
`bun scripts/native-code-fps.ts` opens a native code-editor window for a paired
comparison of token text hosts and cached original-font row images. It verifies
actual scrolling and counts native draws; these are not compositor presentation
timestamps or a claim of sustained full-application refresh-rate parity.
For full-application scrolling, build the release core and display binding, then run:

```sh
GPUI_WAYLAND_PRESENTATION_TRACE=1 NEOISM_BENCH_MAXIMIZED=1 \
  NEOISM_BENCH_SCROLL=mouse-slow NEOISM_BENCH_MS=6000 \
  bun scripts/native-scenario-fps.ts code markdown agent > /tmp/neoism-fps.log 2>&1
bun scripts/summarize-presentation.ts /tmp/neoism-fps.log
```

Repeat with `mouse-regular`, `touchpad-slow`, and `touchpad-regular`. The default
`fast` profile cannot establish slow-scroll smoothness. Compare equal reported
window dimensions and verify focus; maximizing on current Hyprland uses its Lua
dispatcher. Presentation feedback measures displayed frames, while native draw
counts measure submitted work. With VRR, a zero protocol refresh interval leaves
the missed-refresh count unknown; presentation gaps remain available. Optional
`GPUI_GPU_TIMING_TRACE=1` separates surface acquisition, GPU command recording,
and presentation call durations, excluding asynchronous GPU execution.
`NEOISM_GPUIX_PROFILE=1` measures bridge operations and image uploads. Run timing
tests without a concurrent build and record power/display settings alongside them.
Fixture timings do not establish live-chat performance. Validate the packaged
application with the user's actual workspace, configuration, and conversation.
`bun run benchmark:frame-paths` compares full and retained palette frames and
editor LSP pointer updates through the release bridge, without a window or
daemon. These are CPU/wire-size measurements, not presentation FPS. Palette
motion retains catalog rows; LSP pointer/overlay updates retain document paint
and leave pending scroll frames intact. Markdown can opt into native canvas
packets instead of converting the same paint commands again in JavaScript.
Display callbacks and each daemon reply publish their reactive changes in one
batch. Profiling includes `animation.batch`, `daemon.receive.<family>` (parsing,
request delivery, subscribers, and synchronous reactive flush), and
`daemon.agentBatch` (deferred Agent FIFO processing), without message contents.
Background Agent text/history events still hydrate their session cache but do
not request a repaint of the viewed transcript. Lifecycle and family activity
events retain their redraws so conversation-header status stays current.
The display clock reserves one successor callback before preparing an animation
frame, so mutation flushing cannot postpone registration past the UI thread's
next frame opportunity. A final empty callback drains that reservation without
advancing animation or requesting another wake; native tests verify that these
callbacks do not force redraws. `NEOISM_BENCH_LOOKAHEAD=0` runs the previous
scheduling policy in `benchmark:fps` for comparisons. `NEOISM_BENCH_CHAT_ONLY=1`
isolates transcript rendering, and `NEOISM_BENCH_HIDE` accepts comma-separated
test IDs to omit from layout in the benchmark process. These diagnostic runs
do not represent the complete application.
Session initialization also preloads the configured chat/Markdown font library
on a background thread. Painters share the completed library by font settings;
the first chat mount can reuse it instead of loading fonts on the UI thread.
The base picker font library is preloaded too. Picker effects track only their
own inputs and share one initial paint; query and placement changes do not
resend the catalog. Model/agent picker opens reuse successful catalogs for
30 seconds, while explicit refreshes still fetch immediately and concurrent
refreshes share a request. `/connect` displays providers before fetching auth
methods for the selected provider.
The retained chat painter preserves fractional vertical positions through the
scroll tail, avoiding whole-logical-pixel pauses and jumps on scaled displays.
Sending inserts the user message and clears the composer immediately, before
session creation or prompt acknowledgment. The same message ID reconciles the
server echo; rejection removes only unconfirmed messages and restores the draft
without erasing newer typing. A temporary local timeline is retired when the
new session adopts it. First submission leaves home immediately, including
through empty initial history replies. Head
history refreshes apply the server's ordering while retaining off-page history
and live rows, including reasoning that arrived after its answer.
`SIGUSR2` writes read-only timeline offsets, content/viewport dimensions, visible
source ranges, and animation reasons alongside the existing state dump. Check
these before and after scroll samples: clamped boundaries and demand-driven idle
frames must not be interpreted as scrolling performance. The conversation
catalog schedules only its own animations, excluding the invisible home pane.
`NEOISM_GPUIX_SCROLL_TRACE=1` logs frame intervals, delivered scroll input,
timeline metrics, and median displacement of matching text runs (without
message text) to distinguish missed deadlines from layout-position jumps.
Raster text exports the original renderer's already-emitted glyph instances;
it no longer repeats cold glyph work through a second font engine.
The mounted chat canvas retains up to 16 MiB of recently visible raster textures
to avoid re-uploading text on scroll reversal. LRU eviction releases only the
cache's reference; visible canvases retain their own leases. Unmounting releases
the cache, and oversized images keep only their normal frame references.
ASCII blank text advances stay in the original layout and selection data but
do not become fallback canvas text commands that prevent paint-cache reuse.
Restored chat tabs request missing history even when pane focus already marked
the tab active. History delivery identifies head and continuation replies
explicitly: head refreshes retain loaded older pages and their cursor, while
continuations prepend their bounded page, including replies to parked sessions.
End-to-end frame-rate parity still needs measured native-window comparisons. The terminal check creates
and closes its own PTY. Other interactive checks use isolated daemon fixtures or
read-only operations. Linux GPU screenshot capture requires compositor tools;
personal captures and generated binaries are ignored.

Packaging produces `dist/neoism`, the native bridge, libghostty, and its
license. Keep these files together. GPUIX, fonts, and the wordmark are embedded.
Override library paths with `NEOISM_GPUIX_CORE` and `NEOISM_GPUIX_GHOSTTY` if needed.

Element geometry queries read a renderer-local snapshot published after paint.
They no longer queue a synchronous UI-thread command from menu effects, pointer
handlers, or keyboard cursor positioning. The snapshot includes translated and
deferred content and drops boxes that did not paint in the completed frame.
