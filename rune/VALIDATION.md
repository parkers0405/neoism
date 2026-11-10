# Rune validation — November 9, 2026

The preview runs separately from the installed app. No daemon/protocol source changes or packaging cutover
are included. The native tests use real Wayland windows, actual keyboard/wheel input, Rune element traces,
and compositor presentation feedback. All timings below are short local samples, not a guarantee for every file.

## Editor scrolling regression

The earlier editor built and laid out the entire wrapped file on each rebuild. Narrow panes greatly increased
the row count. The replacement caches syntax and wrap geometry and uses fixed-height virtual rows to mount only
the viewport and overscan. Text and wrap-width changes invalidate the cache; scrolling and cursor movement do not.

These wheel captures used normal redraw scheduling (`--natural`), with no forced frame count or concurrent build.
Both narrow tests used `editor-long`, a 624×674 window, the file tree open, and the same wheel workload.
Presentation times were compared from 2.0 to 4.4 seconds after the first presentation:

| Build | Presentations | Elapsed | Missed refreshes | Median interval | 95th percentile |
| --- | ---: | ---: | ---: | ---: | ---: |
| Earlier preview | 113 | 2.375 s | 173 / 60.70% | 25.00 ms | 25.02 ms |
| Virtualized editor | 288 | 2.392 s | 0 / 0% | 8.33 ms | 8.37 ms |

The updated trace traversed 2,929 logical pixels and passed cursor clipping checks through reversal and settling.
The trace checker reads Rune's optional visible rectangle and allows the documented subpixel omission plus pixel
snapping tolerance; the unbounded cursor's layout rectangle can correctly lie outside its clipped parent.

`editor-stress` contains 20,000 Unicode source lines that wrap in a split pane. Its normal-scheduling wheel capture
had 203 presentations over 1.692 seconds and one missed refresh (0.49%) in the 2.8–4.5 second scrolling interval.
CPU submission work during that interval was 2.46 ms at the 95th percentile and 2.70 ms maximum. Initial layout of
that synthetic large file took about 232 ms; the cache removes that work from scrolling, not from first open.

Captures on this machine are under `/tmp/neoism-rune-verify/`:

- `native-editor-old-narrow.{jsonl,wayland.log}`
- `native-editor-new-narrow.{jsonl,wayland.log}`
- `native-editor-stress.{jsonl,wayland.log}`

These temporary artifacts are reproducible with the scripts; they are not committed binaries or recordings.

## Functional checks

- 96 Rust tests pass, including completion edits/Unicode/undo, layout cache reuse/invalidation, mouse selections,
  Vim behavior and message-version invalidation.
- Headless scenes cover editor, LSP hover/completion, split panes, agent transcripts, menus, settings, Database,
  narrow layouts and the large wrapped editor. Input checks cover completion acceptance/dismissal, word/line
  selection, multiline paste, editing an unfocused split, shortcut capture/conflicts and selection autoscroll.
- Real daemon tests returned diagnostics, hover and completions for Rust, TypeScript, JavaScript and Python.
  Local Rust/Python server executable problems were repaired through the existing LSP configuration and checked
  against the normal running daemon. Other supported languages have not each been exercised live.
- A live agent turn streamed through the daemon and rendered text/code. `/usage` read account windows from the
  existing local HTTP endpoint. A scratch SQLite database was edited and read back from disk.
- Bounded native terminal-output and simulated-streaming runs each recorded 657 presentations with zero missed
  refreshes over about 5.47 seconds. Those earlier runs forced submissions with `--frames`; they establish the
  sampled rendering throughput, not normal redraw scheduling under every workload.
- A subsequent clean long-transcript wheel run used normal scheduling after Markdown memoization. Its active
  2.8–4.5 second interval recorded 204 presentations over 1.692 seconds with zero missed refreshes; the trace
  included 7,556 logical pixels of scroll. Capture: `native-report-natural.{jsonl,wayland.log}`.
- Normal-scheduling follow-ups also passed: streaming recorded 456 presentations over 3.792 seconds (interval
  1.0–4.8 s); terminal output recorded 288 over 2.392 seconds (interval 1.8–4.2 s). Both had zero missed refreshes.
  The terminal workload's completion marker confirmed all 360 updates ran. Captures are
  `native-streaming-natural.{jsonl,wayland.log}` and `native-tui-natural.{jsonl,wayland.log}`.

## Outstanding parity work

The screenshot suite is a reproducible regression baseline, not an assertion that every reference pixel matches.
Longer mixed workloads remain to be measured. The 20,000-line first-open cost also remains visible in the trace.

Rendered Markdown editing is deliberately deferred. Database filtering/schema detail are implemented and checked
against a scratch database; their full visual parity and complete rebinding coverage still need comparison with the reference. The installed launcher, release packaging, and GPUiX removal remain
the separate cutover step, after parity is established.


## Workspace, pointer and database follow-up (2026-11-09)

The last completed build passed 106 Rust tests. The tests include save acknowledgements/failures, editing while
saving, workspace draft/focus preservation, adjacent/final workspace close, and agent `q` versus `qall`.
Headless input checks also exercised Ctrl+Shift+Left/Right, Ctrl+Shift+W, workspace `+`, Alt+1 restoration,
closing an inactive tab through its hover X, and dirty-tab confirmation.

Each workspace now owns a daemon socket/inbox and independent tabs, split layout, editor buffers, terminal
state, conversations, sidebars and focus. Workspace creation/claim/switch use the existing protocol. Closing
uses `CloseWindowWorkspace` and waits for its acknowledgement before selecting the neighbour or quitting the
last window. It does not use the global `CloseHostWorkspace` operation. A separate ephemeral daemon verified
creation, close acknowledgement, editor restoration and subsequent LSP hover.

The scrolled mouse-hover bug was a coordinate-space mismatch: virtual rows rebase their local origin, so local
pointer coordinates were interpreted as document coordinates. Mouse hit testing now derives document positions
from window coordinates, the editor viewport and its scroll offset. A Rust reproducer previously requested line
61 while Shift+K requested line 150. Both now request line 150. TypeScript, JavaScript and Python checks also
returned identical nonempty documentation for mouse and keyboard hover at line 150. These tests used actual
language servers through the daemon. Captures/replies are in `/tmp/neoism-rune-workspaces/hover-{ts,js,py}.*`.

Inspection of those PNGs also found a second wrapping pass inside virtual code rows. Giving the prewrapped
rich-text fragment the remaining row width prevents short comments from wrapping onto the following code line.
Tab file glyphs now use the same font, mapping, size and colors as tree glyphs, including inactive tabs.

Database checks inserted an untouched draft row into a scratch SQLite table and read back its generated id,
`Untitled` default text and integer-zero default. The typed `done=1` filter returned only the `Second` row.
Schema details, explicit NULL versus omitted/default columns and cell value types are represented in the UI.

Rust builds were paused at the user's request. No Cargo, rustc or Rust linker was running when checked, and no
new compilation was started after that request. The preview on Hyprland workspace 4 uses the last completed
binary. The native keyboard test stopped when focus changed; it does **not** establish native navigation or
final-window-close success. No new performance claim is based on these headless runs.

Source changes after the last completed test compilation still need a build/test pass once builds are allowed:

- Wait for the current finder query before Enter opens a result; cover immediate Enter against stale results.
- Discard remaining old-socket replies after a workspace close acknowledgement selects another workspace;
  cover an old file reply sharing the next workspace's path.
- Prevent new workspace creation while its predecessor's close is pending and reject invalid close indices.
- Select the owning pane immediately when reopening an already-open file/config/conversation.
- Give confirmation modals keyboard ownership over focused composers.
- Dismiss hover/completion on scrolling, including precise scrolling, and rebuild when a visible popup closes.

The new regression tests and syntax checks are saved, but the additional tests were not compiled/run after the
build pause. Full parity, durable workspace restoration, remaining terminal/keybinding coverage, longer native
workloads and the packaging/installed-binary cutover remain outstanding. GPUiX remains the shipping reference.


## Current user corrections (build pause lifted)

- [x] Space+x closes the current Normal-mode editor buffer, respecting unsaved edits.
- [x] Home wordmark hover animates; preserve the working agent hover and animate unfocused visible panes.
- [x] Tab and workspace close buttons appear only while hovered, without changing tab widths.
- [x] Holding and dragging either tab row reorders its tabs; cancellation preserves order.
- [x] Shift+Alt+Left/Right reorders the focused tab; workspace focus (or Ctrl as in the reference) targets workspaces.
- [x] Restore MCP-supplied icons in /mcp, its actions header, and timeline tool calls; retain status indicators and bundled/fallback marks.
- [x] Run Rust tests, headless geometry/input checks, native window checks, and refresh the preview on space 4.

The user explicitly allowed Rust builds again after these corrections were reported.


Validation after the build pause was lifted:

- `build.sh test --release --quiet`: **119 tests passed**. Reorder tests preserve workspace
  identities/drafts and pane boundaries; MCP tests exercise supplied PNG/JPEG, bounds/fallback,
  catalog ingestion, history and nested tool compositions.
- `check-headless.py`: all reference scenes and input regressions passed. Updated the identifier
  double-click coordinate after the earlier geometry fix (the old coordinate hit parentheses), and
  checked rebinding by its opened palette instead of expecting an off-screen shortcut row.
- `check-hover-and-close.py`: Home/agent hover, both unfocused split hovers, hover-only close buttons,
  and clean/dirty Space+x passed. Close-button placeholders keep strip geometry unchanged.
- `check-tabs-and-mcp.py`: actual pointer drag and keyboard reorder passed for both strips; outside
  drops preserve order. A custom orange/teal server image is checked by pixels in the picker,
  actions header and timeline. This proves server-supplied artwork, independent of bundled brands.
- `check-native-tabs.py`: native Hyprland window, real mouse-button/motion input and keyboard chords,
  per-frame element trace. Both tab strips reorder, X disappears after hover, and Home/agent letters
  grow on hover. Hyprland cursor warps alone do not deliver physical motion; the harness now emits
  motion through its temporary uinput device. Screenshot: `/tmp/neoism-rune-corrections/native-tabs.png`.
- Native `editor-stress` wheel run: normal redraw scheduling, 20,000 wrapped source lines in a split.
  Cursor clipping passed across **1,202 frames**, scroll distance **2,419.64 pt**. The active interval
  from 1.8–4.1 seconds had **276 presentations in 2.292 s**, **zero missed vblanks**, p95 **8.378 ms**
  at 119.99 Hz. This remains a short sample, not proof against every intermittent stall.
- Updated preview launched on **Hyprland workspace 4**, PID **3272412**, binary
  `/tmp/neoism-rune-preview/neoism-rune-corrections`. The older preview has been left intact because
  it may contain user drafts. The installed frontend and packaging have not been switched.

Logs: `/tmp/neoism-rune-final-tests.log`, `/tmp/neoism-rune-full-headless.log`,
`/tmp/neoism-rune-hover-check.log`, `/tmp/neoism-rune-tabs-mcp-check.log`, and
`/tmp/neoism-rune-native-tabs-check.log`. Native traces are under `/tmp/neoism-rune-corrections/`.


## Palette close regression and GPUiX tab motion

The previous Agent-composer `q` check missed the command palette: Alt+P → `q` → Enter
fuzzy-matched **Quit** and exited the native window. Reproduced that exit with Wayland
keyboard input against the previous binary. Exact GPUiX aliases now resolve before fuzzy
ranking: `q`/`quit`/`quite`/`close` close the tab, `qa`/`qall`/`quitall` close its workspace;
optional `:` and explicit `!` are recognized. The palette shows the concrete close target.
The Agent composer shares this parser. `q` on the last tab leaves Home; `qall` selects
an adjacent workspace and exits only after closing the final workspace.

The drag treatment now follows GPUiX's `TabSlide`: activate on press, hide the source
while held, move a plain floating tab with the pointer, reorder immediately when its
center crosses its logical slot boundary, and critically damp the neighboring slides.
No insertion line or outlined ghost remains. Workspace ghosts use the same folder icon
as their source. Overflow strips scroll to reveal keyboard-selected tabs and clip the
chrome cursor. Switching workspaces preserves the window-scoped strip measurements.

Validation:

- **121 Rust tests pass**, including every exact close alias and workspace geometry retention.
- `check-native-close.py`: real keyboard checks pass both offline and with the daemon
  connected. Agent `q`, palette `q`, last-tab `q`, successive `qall` with neighboring
  workspaces, and final-window `qall` all have their expected scopes.
- `check-headless.py`: reference surfaces and input regressions pass, including new
  palette `q` and `qall` checks.
- `check-tab-motion.py`: both strips' frame traces show live reordering, intermediate
  neighbor positions, no spring overshoot, hidden source, pointer-following ghost and
  ghost removal on release. Rendered PNGs inspected in `/tmp/neoism-rune-tab-motion/`.
- Native tab checks use an actual temporary uinput pointer. The harness waits for a
  delivered hover before pressing, because a compositor cursor warp is not proof that
  Rune received pointer motion. Drag/reorder chords and both wordmark hovers pass.

Logs: `/tmp/neoism-rune-close-final-tests.log`, `/tmp/neoism-rune-native-close-offline.log`,
`/tmp/neoism-rune-native-close-connected.log`, `/tmp/neoism-rune-close-headless.log`,
`/tmp/neoism-rune-close-motion.log`, `/tmp/neoism-rune-native-tabs-final.log`.
The older previews listed above have since closed. The installed GPUiX app is unchanged;
full parity, longer workload checks, durable workspace restoration and cutover remain open.

Updated preview: PID **3396019**, class `ai.neoism.rune.preview.close-motion`,
`/tmp/neoism-rune-preview/neoism-rune-close-motion`, verified on **Hyprland workspace 4**.
`check-tabs-and-mcp.py` also passes with the revised motion: reorder, outside drop,
keyboard navigation, overflow reveal, and supplied MCP artwork in all three surfaces.

A subsequent 30-second native wheel run stopped when desktop focus changed; the input
guard correctly stopped sending wheel events. It is **not** a completed sustained-scroll
measurement. The earlier short presentation samples remain the measured performance evidence.
The native harness now accepts `--wheel-seconds` for a future uninterrupted longer capture.

## Large-file open and edit cost (Claude, 2026-11-09)

`editor-stress` (20,000 wrapped Unicode lines in a split) used to spend two ~180 ms frames on open: one
wrapping every line at a guessed 900 pt width before the view was measured, then again at the real width.
Every keystroke also re-wrapped the whole file (~150 ms), since the layout cache was keyed on the revision.

- The code view mounts no rows until the view has reported its width, so the guessed-width pass is gone:
  first frame 180 ms -> 7 ms; the one real wrap pass (~150-180 ms for this pathological file) remains.
- Wraps are cached per distinct line text (hash) at the current width, so an edit re-wraps only the lines
  it changed wherever the others shifted to; highlighting stays whole-file (~2 ms here).

121 Rust tests and `check-headless.py` pass.


## Frame pacing in real chats (2026-11-10)

The agent tab dropped frames whenever something moved. Measured on the 144 Hz panel with a
real conversation loaded from the running daemon, real wheel input and compositor
presentation times, the pulled branch scrolled at 89 to 123 fps (14 to 38% of refreshes
missed) on a busy machine. The causes, in the order they were found:

- The transcript's width handler re-ran the views on every frame of a scroll (about 4 ms
  each): a layout handler inside a scroll view reports every frame.
- The vsync heartbeat (every 300 ms) and the caret blink (every 530 ms) each re-ran the views.
- A running tool's spinner re-ran them on every frame for as long as the tool ran.
- Every commit rebuilt all mounted rows but finished replies: each tool card (re-parsing its
  diff, twice over in the row list) and each question, whose orb was 100 to 150 live-coloured
  elements. Orbs were most of a transcript's elements (1,354 layout nodes in `question-rail`
  against 275 as pictures).
- Every text delta of a streaming reply was its own commit.

Same workloads on both builds (`rune/scripts/check-live-pacing.py`; the baseline is the
pulled branch with only the test hooks added). UI-thread work per frame in ms, then refreshes
missed on the real panel with the machine otherwise idle:

| Workload | Before: p50 / p95 / max | After: p50 / p95 / max | Before missed | After missed |
| --- | --- | --- | ---: | ---: |
| Wheel scroll, long real chat | 1.8 / 3.3 / 4.8 | 0.4 / 1.0 / 3.5 | 0.3% | 0.7% |
| Trackpad scroll | 1.2 / 3.3 / 6.9 | 0.4 / 1.0 / 3.8 | 0.8% | 0.0% |
| Long live run, scrolling | 1.8 / 2.7 / 4.6 (every frame a commit) | 0.4 to 0.9 / 1.4 to 2.8 / 5.7 (none) | 0.0% | 0.0%, 0.0%, 2.2% |
| Long live run, fast reply streaming | 2.8 / 4.2 to 5.4 / 7.6 to 35.7 | 0.7 / 3.6 / 6.1 | 2.3 to 6.9% | 0.4 to 0.9% |
| Wheel and trackpad, all 24 cores busy | 0.7 to 2.5 / 4.9 to 5.9 / 11.3 | 0.6 / 1.3 to 1.7 / 6.1 | 12.7 to 22.4% | 6.2 to 8.5% |

A real prompt (two `sleep` tool calls, a file read and a 600-word reply) re-ran the views
1,003 and 683 times in its first two 14 s spans before and 68 and 11 after.

The band of nothing between a streaming reply and "Crafting" was geometry, not pacing: the
transcript kept a fixed 200 pt under its last row while the pinned line sits just above the
composer, 41 pt lower than the row it stands for. The rows now end level with the composer's
top (measured), with the same 41 pt of room after the last row at rest. The pinned line and
the streaming row's own reveal are as they were.

What remains: a commit still costs 2.5 to 4 ms in a long transcript (Rune's layout, apply and
paint of the growing reply), so the heaviest streaming case misses a refresh a few times in
ten seconds; the cursor trail and the wordmark effects still re-run the views while they
play; and with every core saturated, scheduling alone drops frames no app change recovers.
The headless output is more forgiving than the panel: judge by work per frame there.

## History paging, jump to latest, wide tables (2026-11-10)

Older history used to load only after the reader stopped near the top (one page per fresh
gesture within a viewport of it), the flick was cut when the page landed, and the view then
jumped: the page's rows are measured the frame after they land, Rune moves the offset to keep
the reader's row, and the scroller's absolute offset for that frame was applied again on top
(-19, +45, +76 and +234 pt over six pages of a real conversation).

The transcript is now scrolled by a held row, and pages are asked for three viewports ahead of
a reader heading up. One continuous nine second wheel scroll through the same conversation
(`NEOISM_RUNE_TRACE_SCROLL=1`): 229 to 559 messages, eight pages, each landing with the reader
1,900 to 3,200 pt from the start; the content's end moved in 28 frames and the view moved by
anything but the scroll step in none. The reader reached the start only once the whole history
was loaded.

Away from the end, a jump-to-latest button sits above the composer's right edge (the
terminal's `block-jump-latest`). A table wider than the message scrolls sideways (trackpad, or
Shift with a wheel) with GPUiX's hover scroll bar: along the table's bottom edge, held just
above the composer while the table runs on under it, draggable. Checked headlessly with a six
column, thirty row fixture: hover, a sideways swipe, the bar above the composer, a drag.

## Code and Markdown files (2026-11-10)

Scrolling a file re-ran the views on every frame: the block cursor is drawn at the window's
root, its place followed the text's scroll offset, and every new place was a commit (and a
frame-late, spring-chased one: the cursor trailed after its own character). Each commit also
rebuilt every mounted row, some 160 of them with their highlight spans.

- The cursor rides the scroll: when the file under it scrolls, the trail and its target move
  by the same distance at once (`Cursor::carry`), and a block that only travelled since the
  views built it moves on a live offset (`Cursor::place`). Views re-run for a new shape (a
  trail), a new clip, or once when a ride ends.
- Each visual row is a memo keyed by its own text, wrap and highlights (hashed), the cursor
  line, selection, search, yank flash, diagnostics, theme and type: a scroll's row-mount
  commits and a cursor move build the rows that changed, and typing builds the line typed on.

`rune/app/src/editor/vim_apply.rs` (1,370 lines) and `rune/README.md` on the headless output,
UI-thread work per frame in ms (`NEOISM_RUNE_DRIVE` wheel, trackpad and `keys` steps):

| Workload | Before: p50 / p95 | After: p50 / p95 |
| --- | --- | --- |
| Rust file, wheel | 4.0 to 4.4 / 8.2 to 10.9 (90 to 105 fps, every frame a commit) | 0.5 to 0.7 / 2.4 to 2.7 (144 fps, none) |
| Rust file, trackpad | 4.1 / 6.3 | 0.6 / 2.3 |
| Rust file, `j` held | 2.7 / 3.6 | 1.2 / 2.6 |
| README, wheel | 2.1 to 2.3 / 3.6 to 3.7 | 0.6 to 0.8 / 2.2 to 2.6 |

Typing still re-runs the views on every frame (the caret's hop is a new shape each frame) at
about 2.8 ms, and each keystroke re-highlights the whole file in `poll` (2.4 ms in the 1,370
line file); neither is addressed here. Measure with nothing else busy: rust-analyzer
re-indexing after an edit doubled every number above.

## Status pills, diagnostics popup, Ctrl+D, Copy (2026-11-10)

- The error and warning counts are their own rounded pills left of the status chain
  (`DiagPillSpec`), 6 pt apart. As links in the chain a solid red or yellow section ran on
  under the next pill's cap. A press opens the diagnostics popup over the pill
  (`editor/diag_popup.rs`, after `panels/diagnostics_popup.rs`): the severity's diagnostics
  with their lines, Up/Down, Enter or a click to go to the line, Escape or a click outside to
  close. Checked headlessly in the `lsp` scene: both popups, and Enter moving the cursor from
  line 2 to the error's line 3. Long messages truncate; the original scrolls them sideways.
- Ctrl+D in a conversation is the jump-to-latest button.
- A code block's Copy button copies (`Tx::copy`, traced with `NEOISM_RUNE_TRACE_COPY`) and
  cross-fades to a green "Copied" for 1.4 s on live opacities; it is a 26 pt button with a
  hover wash rather than a bare word.

## Servers and SSH systems open their own workspace tab (2026-11-10)

Connecting used to replace the window: one daemon target for every workspace, the local
workspaces stashed until Local Server was picked again. A connection now belongs to a
workspace (`servers::flow::Link`, moved with `workspace::State`): each daemon socket dials
its own target (`Daemon::start_to`), connecting opens a new workspace tab on the server next
to the local ones, picking a server that is already open goes to its tab, Local Server goes
to a local tab, and disconnecting or removing a server closes its tab (and its tunnel). A
tab on a server or an SSH system wears a cloud instead of a folder. Only local workspaces
are written to the recovery snapshot.

Covered by unit tests of the flow (a server tab beside the local one, the daemon's adopted
workspace, the SSH home workspace, re-selecting, disconnecting). Not yet exercised against a
second daemon or a real SSH system in this pass.

