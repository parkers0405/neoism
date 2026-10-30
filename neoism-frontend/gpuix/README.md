# Neoism GPUIX frontend

The sole workspace frontend built by this branch, installed as a separate preview. SolidJS renders through
GPUIX/GPUI; Effect manages the daemon connection and typed requests. The Neoism
TypeScript SDK handles Agent APIs. The legacy desktop, WASM renderer, Rio emulator,
and Lua execution hosts have been removed.

## Run

With the existing Neoism daemon running, from this directory:

```sh
npm ci
npm run setup:runtime
npm run build:core
npm start
```

Blaze is the Rust builder for the services, native bridge, tests, GitHub Actions,
and Nix package. `scripts/blaze.sh` and `scripts/setup-blaze.mjs` install the exact
published release and verify its SHA-256 against `native/blaze-pin.json`.
Root `make build` builds the services, native bridge, application bundle, and
standalone Agent web GUI; root `./install.sh` installs that bundle in
`~/.local/lib/neoism-gpuix/bin`, preserving the existing daily `neoism` launcher.
Application builds
never fall back to Cargo. Windows CI bootstraps Blaze itself from the pinned
source because upstream publishes no Windows archive; its application tests use Blaze.

After building and testing the native bridge, `npm run build -- --reuse-core`
packages that existing library after another FFI smoke check. The normal build
compiles the bridge through Blaze before packaging.
Windows uses upstream's documented `BLAZE_PROC_MACRO_SERVER=0` host macro mode
because its matching native macro server is also unpublished.
The compiler's [documented native macro host](https://github.com/indexable-inc/blaze/blob/8f144c2dae639fd093e4c489dace50132002fe0a/docs/cargo.md) handles SQLx and `enum-as-inner`.
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
GitHub-hosted Linux jobs reclaim unused SDK space and add 32 GiB of swap through
`scripts/prepare-blaze-runner.sh`. This is limited to disposable hosted VMs;
standard runners have [limited memory and disk](https://docs.github.com/en/actions/reference/runners/github-hosted-runners).
After services have built successfully, `bash scripts/gpuix.sh build-frontend`
rebuilds the native bridge and frontend bundle using those service executables.
The native bridge enables mimalloc's `local_dynamic_tls` mode because Bun loads
it into an already running process; reserving static TLS at application startup
is not required.

`npm run dev` enables hot replacement. npm scripts select project-local Bun
1.3.13, which supports Unix WebSockets. `setup:runtime` also works when npm blocks
Bun's install script. Native libraries are replaced atomically.

Packaged builds use Blaze's release profile. Standalone `build:core` defaults to
the development profile; set `NEOISM_GPUIX_BUILD_PROFILE=release` for the packaged
profile. Native outputs use `target/gpuix-core-blaze`, independently of the
services' build directory. `NEOISM_GPUIX_TARGET_DIR` overrides that directory.

The app reads `~/.config/neoism/config.json` (JSONC), discovers the local daemon
socket and token, or connects to `ws://127.0.0.1:7878/session`. It reuses the local daemon and starts a standalone local daemon if none is reachable.
An explicit `--daemon` endpoint never starts a local service.

```sh
npm start -- --workspace WORKSPACE_ID
npm start -- --daemon ws://HOST:7878/session
npm start -- --help
```

`NEOISM_DAEMON_TOKEN` is the upgrade credential; `NEOISM_PAIRING_TOKEN` is the Hello
credential. Local credential discovery applies only to the default endpoint.

## Working paths

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
- Cold startup selects an independent Home tab showing the original
  splash and eight source menu actions. Welcome creates no PTY and never
  captures terminal input. The `+` immediately after the buffer tabs opens another Home tab
  without creating a PTY. The first explicitly opened Terminal owns local cwd
  navigation; every terminal tab can close. Recovered dirty documents stay
  open while Welcome is selected on startup. Original wordmark click effects
  paint across the pane and use a persistent per-surface shuffle bag;
  automatic playback follows
  `appearance.splash-auto-animate` (default false).
- libghostty is the terminal engine inside the existing Terminal tab. Keyboard
  input goes directly to daemon PTYs. Hidden tabs continue parsing output, and
  closing the frontend leaves daemon shells alive. There is no shell composer.
- Configured palettes, fonts, wallpaper/cached blur, original dimensions, and
  animation state are reused. Native clipboard operations use platform tools.

GPUIX owns GPU painting, compositing, and the OS window. Reusable CPU font shaping,
measurement, and glyph rasterization were migrated into `neoism-paint`; cached pixels
become GPUI image textures. Shared UI entrypoints use the portable `Paint` interface.
Neither the frontend nor daemon links Sugarloaf, the Rio terminal emulator, the old
window host, or a Lua VM. `neoism-customization` retains only styling/contribution data.

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
