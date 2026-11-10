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
