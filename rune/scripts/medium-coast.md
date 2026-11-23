# Native medium scroll check (macOS)

Use a visible window on a 120 Hz display. `question-rail` is an offline 18-message text fixture. The setup drag leaves room on both sides. `hand:4:medium` runs one pair, older then newer: 48 finger reports over 0.4 s, 250 points, 450 pt/s release, then exponential decay (0.42 s) to 22 pt/s. Both strokes use the same whole-point quantizer and 120 Hz deadlines; the seed is unused. Four seconds is the minimum drive slot, not a coast cutoff.

```sh
rune/app/build.sh build --release
bin="${CARGO_TARGET_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/neoism-rune-target}/release/neoism-rune"
out=/tmp/medium-after  # use a fresh name for each run
python3 rune/scripts/check-live-pacing-macos.py --binary "$bin" --scene question-rail --size 1252x674 --drive 'wait:1;touch:600:0.6;wait:1;mark:pair;hand:4:medium;mark:end;quit' --env NEOISM_RUNE_TRACE_SCROLL=1 --out "$out"
python3 rune/scripts/analyze-medium-coast.py "$out.log"
NEOISM_RUNE_OFFLINE=1 "$bin" --scene question-rail --size 1252x674 --at 1 --scale 1 --snap "$out.png"
```

Read the `older` line for the first coast. The analyzer takes the last 500 ms ending at the last completed frame before each native momentum-end marker. It counts drawn frames with a conversation `to=Some(...)`, and intervals of at least 1.5 refreshes between consecutive such frames wholly inside that window. These are placement gaps, not missing Metal presentations: presentation intervals are reported separately from Metal timestamps. The test excludes setup, rest and direction changes. It rejects edge placements, covered frames in the window and captures without 120 Hz presentation evidence. This tests native event handling and text placement; it is not a physical trackpad capture or a pixel-motion detector.

Two-frame baseline (`scroll.rs` blob `7146a0fed63ba5c434897cc0cf35654918d3bf6e`, Apple M5 Pro): first coast 43/61 frames placed, 10 gaps, worst 33.24 ms; reverse 40/61, 11 gaps, worst 34.11 ms. Worst presentation intervals were 8.33 and 8.34 ms respectively. Native event delivery relative to vsync varies between runs even though the input sequence is fixed.