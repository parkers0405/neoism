# Neoism terminal frontend

**Status: paused, opt-in prototype.** Keep this frontend out of normal builds, CI builds, release packages, and installation until explicitly resumed. The source and manual development launcher remain available; do not automatically build or install its dependencies from the GUI pipeline.

A separate SolidJS + OpenTUI frontend for the Neoism workspace daemon, using the same Rust editor, Vim engine, collaborative document/save path, LSP services, JSONC config, theme tokens, and daemon transport as the GUI. This is not the retired agent CLI TUI; its entrypoints and launchers remain untouched.

## Run

Requires an interactive terminal, the existing Neoism native editor library, and an existing workspace daemon. Dependencies include a pinned Bun 1.4.1 runtime. From the repository root:

```sh
bash scripts/tui.sh
bash scripts/tui.sh --offline
```

The second command previews the actual splash/chrome without connecting. From this directory, `bun start` also works. For a fresh checkout, install dependencies with `npm install` in this directory (or `bun install` with Bun already installed). Native-library lookup is shared with GPUIX; `NEOISM_GPUIX_CORE` can override the library path.

Select a directory or an existing workspace on the daemon used by the GUI:

```sh
bash scripts/tui.sh .
bash scripts/tui.sh --daemon ws://127.0.0.1:7878/session --workspace WORKSPACE_ID
```

Those `bash scripts/tui.sh` commands run from the repository root. The development Cargo launcher also accepts `neoism tui [options]`; this does not yet add the subcommand to an already-installed standalone GUI executable. The default connection uses `NEOISM_DAEMON_SOCKET`, the standard Neoism runtime socket, or `NEOISM_DAEMON_ADDR` (default `127.0.0.1:7878`). An explicit endpoint can also be supplied through `NEOISM_TUI_DAEMON_URL`. The TUI currently attaches to an existing daemon, rather than starting another service. `--workspace` selects an existing ID. A positional directory selects a matching workspace or creates one through the daemon; remote directory arguments must be absolute host paths. With neither, the first workspace owned by the connected daemon is selected.

Authentication follows the existing GUI transport: `NEOISM_DAEMON_TOKEN` is the legacy WebSocket upgrade credential, while `NEOISM_PAIRING_TOKEN` belongs in the Hello message. Default local connections can discover the standard local daemon-token file. Explicit endpoints never inherit a discovered local token. Do not put credentials in shell command arguments.

## Current behavior

- Workspace and buffer tabs inside the content column next to Files, with a full-width divider and status bar below. No extra brand/action/connection row or duplicated workspace heading is inserted above the content.
- Welcome splash area-samples the exact GUI PNG's alpha into explicitly colored half-block cells, keeping empty pixels transparent to the actual theme background. This uses ordinary ANSI text because Neoism's embedded Ghostty acknowledges Kitty graphics probes while its frontend does not paint the image placements; an upload acknowledgment was not proof the splash was visible. Quick actions use equal-width columns and aligned shortcuts, without a version footer.
- The same Nerd Font file glyphs/colors as the GUI are used in Files, tabs, breadcrumbs, search, and chrome. Use a Nerd Font in the hosting terminal, as for NvChad. Files supports hover highlighting, wheel scrolling independently of editor focus, and keyboard reveal through a stable virtualized viewport. Labels are clipped, single-line, non-scrollable text; they cannot consume wheel input or carry a stale wrapped-text offset into a reused row.
- Exact bundled GUI palette values and version from `gpuix/src/design/generated.ts`. The regular Neoism JSONC config is shared: `appearance.theme`, `editor.vim-mode`, and `keybinds.keys` are read and hot-reloaded. Theme selection and Vim toggles update the same grouped file, preserving comments and unrelated settings.
- Existing daemon authentication, request correlation, timeout, heartbeat, and reconnect through `@neoism/service-client`, also consumed by GPUIX.
- Workspace and buffer tabs with active-tab reveal, disambiguated labels, dirty markers, Home, and retained document/cursor/scroll state across workspace switches and refresh. Breadcrumb folders reveal their path in Files.
- Editable buffers use the same Rust engine through a shared Bun FFI transport. Vim motions, counts, operators, visual selection, registers, undo/redo, insert/standard input, incremental search, and supported Ex commands are not reimplemented in TypeScript. Terminal paint consumes source spans and measures grapheme cells independently of GPU pixel positions.
- CRDT open/apply/reconnect/save reuse native `EditorCollaboration`/`CodeDocBinding`. Pending updates are sent before `SaveBuffer`; saves use the daemon as authoritative writer. Dirty close/quit checks include buffers hidden in other workspaces. New files are created through daemon `Files.CreateFile` before CRDT binding.
- LSP open/change/close, diagnostics, hover, completion, signature help, definition/references, document symbols, rename, code actions, and formatting use existing daemon services. Hover/completion popups are cell-anchored, and stale query results are rejected. Completion snippets insert their default text; snippet placeholder navigation is not yet implemented.
- NvChad-derived bottom status follows the GUI's mixed geometry: mode cap, colored cwd text on the dark surface, and a colored folder tail; Git/LSP sections have rounded outer icon edges with straight internal joins. It uses the GUI's mode colors (Normal blue, Insert magenta, Visual cyan), actual diagnostic/ruler/progress data, `~` abbreviation, and a full-width border above. Workspace and buffer tabs remain flat active surfaces, with colored file icons, protected close/dirty targets, and separate visible keyboard focus. Inactive chrome uses the selected theme background, including true black for `pastel_dark`.
- Commands, file/content search, and Ex inputs use Noice-style separate prompt/results windows, with black themed surfaces, selection glyphs, and bounded scrolling. Themes have a split picker: searchable list on the left and the GUI's exact eight-line Rust code preview on the right, stacked on narrow terminals. Token runs are extracted from the shared native painter, not a second sample or mock editor/status display. Arrows or hover preview without changing config; Enter or a row click applies. No success/status fields are fabricated.
- Keyboard and mouse navigation, searchable command palette, resize handling, and a full-width Files view on narrow terminals.

| Key | Action |
| --- | --- |
| Alt+E | Focus/toggle Files |
| Tab | Indent in the editor; move between Files and Home when not editing |
| Up/Down or j/k | Move within Files |
| Enter/Right or l | Expand a directory or open a preview |
| Alt+Left / Alt+Right | Traverse focused tab strips; move between Files and content |
| Alt+Up / Alt+Down | Move between content, buffer tabs, and workspace tabs using the GUI focus ladder |
| Left/Right, Enter, Escape in tabs | Move focus cursor, activate selection, leave strip |
| Alt+P or F1 | Command palette |
| Alt+S or Ctrl+P | Search files |
| Alt+Shift+S | Search project contents |
| Ctrl+G in search | Switch file/content search |
| Alt+D | Change declared workspace directory |
| Ctrl+Shift+W | Create workspace at a daemon-host directory |
| Alt+0 | Home |
| Ctrl+Tab / Ctrl+Shift+Tab | Cycle Home and buffers where supported |
| Alt+] / Alt+[ | Next/previous workspace |
| Ctrl+S | Save current buffer |
| Ctrl+W | Close buffer with unsaved-change protection |
| F5 | Refresh workspace tree without losing buffers |
| K / Shift+F1 | Hover documentation (`K` in Vim Normal mode) |
| gd / F12 | Go to definition |
| gr / Shift+F12 | Find references |
| Ctrl+Space / Ctrl+Shift+Space | Completion / signature help |
| F2 | Rename symbol; Enter confirms project edits |
| Ctrl+. | Code actions; choosing one applies it |
| Alt+Shift+F | Format document |
| Alt+Shift+Space | Toggle and persist Vim mode |
| Ctrl+Q | Quit with unsaved-change protection |

In Vim Normal mode, `/` and `?` use native incremental search, `:` opens the Ex command input, and `:w`, `:wq`, `:q`, `:q!`, `:wa`, `:qa`, `:wqa`, `:bn`, `:bp`, `:e path`, and supported substitutions route through the editor/model. Escape belongs to the editor unless a popup is active. Bracketed paste is sent through native bulk insertion. Clipboard writes use OSC 52 when the terminal supports it; Vim register paste stays native.

The splash only advertises implemented actions. Agent/Notes surfaces, the full interactive Git panel, embedded PTY panes, and split-pane rendering are not integrated yet. The full GUI menu should return as those actions become functional. Only the current host's workspaces are routed by this frontend; remote-host multiplexing and standalone release packaging remain separate work. Text files larger than 2 MiB and binary files are rejected after the daemon response arrives.

## Reuse and next boundary

The production daemon transport and wire schemas were extracted to `../service-client`; GPUIX retains wrappers that inject its profiling and Solid batching. No separate TUI protocol or networking implementation was introduced. The TUI also imports the GUI's pure generated design data and daemon-host path helper.

The chrome focus ladder is shared with GPUIX in `../chrome-navigation.ts`, including the original Rust tab-wrapping policy. Tab navigation and activation are separate: moving the focus cursor does not switch the document/workspace until Enter. Clicking a strip focuses it, and pointer hover or wheel scrolling in Files does not redirect editor keystrokes.

`../native-bridge.ts` now owns the shared FFI transport; the GUI retains its paint/frame adapter and profiling, while the TUI consumes renderer-independent document data. No second editor or subprocess protocol was introduced. UTF-8 byte positions, CRDT UTF-16 positions, and terminal cells remain explicit. `../daemon-executable.ts` shares packaged/development binary discovery without importing GUI modules.

The TUI uses CRDT binding for its documents. Current GUI collaboration eligibility still determines whether a particular GUI buffer joins the same document; attaching both clients to the same daemon alone is not a guarantee of live GUI/TUI edits for every private workspace. The GUI eligibility policy was not changed as part of this frontend.

An embedded shell also requires terminal emulation in the client, not just appending PTY bytes to a text widget. Startup/discovery should eventually be shared with GPUIX rather than grow another daemon launch policy.

## Verify

```sh
bun run typecheck
bun test ./tests
NEOISM_TUI_TEST_FRAME=1 bun test ./tests/App.test.tsx --test-name-pattern 'wide and narrow'
```

Tests cover actual OpenTUI rendering and key input, wide/narrow resizing, Unicode/tab/cursor cells, shared JSONC persistence/reload, workspace/buffer retention, search cancellation, native Yrs synchronization, save ordering, and LSP payload/race handling. Integration tests launch an isolated real daemon and use installed `clangd` for actual hover/completion/definition/diagnostics and unsaved text updates. They also verify authoritative disk saves, offline reconnect, new-file/workspace creation, and Git status subscription. These tests require the built native library and daemon plus `clangd` and Git; `NEOISM_GPUIX_WORKSPACE_DAEMON` can select the daemon artifact. They never attach to the user's daemon or alter the user's config. Standalone packaging and cross-platform runtime behavior remain unverified.