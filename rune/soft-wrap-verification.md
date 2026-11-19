# Code soft wrapping: behavior and verification

## Contract

Wrapping is a display projection, not a formatter. Choose the latest eligible word boundary that fits; use punctuation as ordinary break-before/after classes, never as ranked syntax candidates. Spaces inside strings and comments participate exactly like other spaces. Overlong words hard-wrap at grapheme boundaries. Source bytes, tab columns, syntax spans, selection and search ranges remain unchanged.

Continuation indentation is the source line's leading whitespace (`wrappingIndent: same`), not bracket alignment or an automatic extra indent. If this leaves less than two cells, drop the synthetic indent. The gutter numbers logical lines once and leaves continuation rows blank. Only the caret's visual row receives the active-row wash; the logical line number stays active. Internal break positions belong to the following row; EOL belongs to the final row. Hit testing and vertical movement share this affinity, including clicks in synthetic indentation and beyond the right margin. Margin hits never split combining characters or emoji sequences.

The behavior was checked against VS Code's `src/vs/editor/common/viewModel/monospaceLineBreaksComputer.ts` and `src/vs/editor/common/config/editorOptions.ts` (the complete options/defaults were accessible at tag `1.70.0`: `WrappingIndent.Same`, break-before and break-after classes). This implements wrapping when enabled; it does not claim that VS Code enables `wordWrap` by default. Neoism's theme, font, chrome and scroll policy are retained.

## Checks

From `rune/app` (this host's existing shared cache is `/home/parkersettle/.cache/neoism-rune-target`; `--target-dir` is optional elsewhere):

```sh
cargo check --bin neoism-rune --target-dir /home/parkersettle/.cache/neoism-rune-target
cargo test --bin neoism-rune --target-dir /home/parkersettle/.cache/neoism-rune-target editor::wrap -- --nocapture
cargo test --bin neoism-rune --target-dir /home/parkersettle/.cache/neoism-rune-target editor::view -- --nocapture
cargo test --bin neoism-rune --target-dir /home/parkersettle/.cache/neoism-rune-target wrapped_scroll_cpu_budget -- --ignored --nocapture
```

Focused results: four wrap tests and seven view tests pass; the CPU control passes when explicitly requested. The completed test executable's broader `editor:: --quiet` run passed 139 tests with one ignored control. An earlier run encountered a Markdown link-navigation failure while that file was concurrently changing; the final rerun passed. No release build was used.

The tests exercise greedy breaks in quotes/comments, stable nested-bracket indentation, tabs, narrow widths, CJK, NBSP, grapheme-safe hard breaks and margin hits, unique caret ownership, pointer/caret round trips, j/k visual-row movement, and cache reuse/invalidation. The CPU control asserts actual scroll travel over 5,000 pixels and pointer-identical cached layout on every sampled frame. It steps the production scrollers explicitly because an editor-only Runtime does not run the application's animation heartbeat.

## Headless visual checks

```sh
NEOISM_RUNE_OFFLINE=1 /home/parkersettle/.cache/neoism-rune-target/debug/neoism-rune --scene editor-wrap --snap /tmp/neoism-wrap-wide.png --size 1252x674 --scale 2 --at 1
NEOISM_RUNE_OFFLINE=1 /home/parkersettle/.cache/neoism-rune-target/debug/neoism-rune --scene editor-wrap --snap /tmp/neoism-wrap-narrow.png --size 650x674 --scale 2 --at 1
NEOISM_RUNE_OFFLINE=1 /home/parkersettle/.cache/neoism-rune-target/debug/neoism-rune --scene editor-long --snap /tmp/neoism-editor-reference.png --size 1252x674 --scale 2 --at 1
```

Inspected these PNGs alongside `reference/18-editor-code.png`: editor gutter/breadcrumb/monospace structure retained; continuations use constant source indentation, strings wrap at words, no repeated gutter arrows, and only the visual row containing the caret is washed. The fixture deliberately places the caret on a continuation; changing viewport width moves the highlight with it. The shipping reference uses a different theme/content and includes a continuation arrow; removing that arrow and bracket-position alignment is intentional for the requested VS Code-like behavior, not pixel-for-pixel parity. The fixture also includes combining text, a ZWJ emoji, CJK, nested calls and an overlong identifier. Rune logs an ICU4X Chinese/Japanese segmentation-model warning, although the grapheme geometry tests and rendered sample pass.

## Performance: measured, not a constant-120-FPS claim

Linux, unoptimized debug build, 20,000 logical lines, 600×674 editor-only Runtime, 720 synthetic 120-Hz samples, 15,078.6 pixels actual travel: CPU p50 **4.576 ms**, p95 **5.248 ms**, p99 **5.713 ms**, max **8.364 ms**; **1/720** samples exceeded 8.333 ms. The final frame mounted 407 elements; every sample reused the same full-document CodeCache. There is no per-scroll-frame full-document wrapping or highlighting pass. These values include Runtime layout/update work but not GPU rendering or compositor presentation. An earlier idle-only control was discarded after fixing its missing scroller step.

Full application/headless GPU trace:

```sh
NEOISM_RUNE_OFFLINE=1 NEOISM_RUNE_DRIVE='wait:0.5;mark:scroll-start;touch:-2400:3;touch:2400:3;mark:scroll-end' /home/parkersettle/.cache/neoism-rune-target/debug/neoism-rune --scene editor-stress --size 650x674 --scale 2 --fps 120 --frames 840 --trace /tmp/neoism-wrap-scroll.json --snap /tmp/neoism-wrap-stress.png
```

This completed 840 frames at a **virtual** 120-Hz clock: CPU p50 **2.842 ms**, p95 **7.152 ms**, max **736.410 ms**, with 839 animating frames. Wall throughput was **63.7 frames/s**, not 120. The run includes initial split-width animation and document layout/cache construction (the early trace already contains a 640 ms frame), GPU readback and trace output. Do not substitute that throughput or its virtual clock for display pacing. The CPU control excludes initial layout and proves steady scrolling separately.

Per `../docs/gpuix-live-rendering-baseline.md`, only presentation timestamps establish displayed pacing. Rune's `crates/rune/src/frame/present_time.rs` wires `presentedTime` on macOS and explicitly leaves Wayland presentation-time/Windows DXGI unwired. This Linux run therefore cannot certify constant 120-FPS presentation. A supported native presentation capture on a 120-Hz display remains required; the one over-budget CPU sample also prevents claiming an uninterrupted 8.33 ms CPU budget here. Rune additionally warns that the existing application-owned instant-scroll glide commits the view each frame; this change retains that shared policy rather than changing scrolling for every surface.