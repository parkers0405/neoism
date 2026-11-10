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

Rendered Markdown editing is deliberately deferred. Database filtering/schema detail and full rebinding coverage
still need comparison with the reference. The installed launcher, release packaging, and GPUiX removal remain
the separate cutover step, after parity is established.
