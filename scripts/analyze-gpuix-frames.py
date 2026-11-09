#!/usr/bin/env python3
"""Attribute every missed vblank in a traced GPUiX session.

Launch the frontend with its stderr captured:

    GPUI_WAYLAND_PRESENTATION_TRACE=1 NEOISM_GPUIX_FRAME_TRACE=1 neoism 2> app.log

Optionally add GPUI_WAYLAND_FRAME_TRACE=1 and GPUI_GPU_TIMING_TRACE=1 for UI
thread and GPU submission timings. Then:

    python3 scripts/analyze-gpuix-frames.py app.log [first_line [last_line]] [-v]

Prints the FPS pill readings (same 500ms buckets as the status line), missed
vblanks, and a category for each miss:

  wake-late    the native frame wake reached JavaScript a frame or more late
               (UI thread, compositor or CPU contention, not JS work)
  js-long:OP   one JavaScript operation took most of the frame
  js-late      the wake arrived on time but JS finished after the deadline
  gpu-block    swapchain acquire or present blocked
  ui-long      the UI thread frame itself exceeded the budget
  other        everything was on time; the compositor latched a frame late

Presentation timestamps and `[js-op]` timestamps share the monotonic clock, so
the wake latency is measured against the vblank that preceded it.
"""
import collections
import re
import sys

ROW = re.compile(
    r"\[gpui-presented\] surface=\d+ time_ns=(\d+) refresh_ns=(\d+)"
    r"|\[js-op\] end_us=(\d+) ms=([\d.]+) ([^\[\n]*)"
    r"|\[gpui-gpu-timing\] total_us=(\d+) acquire_us=(\d+) record_us=(\d+) present_us=(\d+)"
    r"|\[wayland-frame-end\] state=\w+ us=(\d+)"
)
SMALL_OPS = {"core.readResponse", "core.decode", "core.encode", "core.parseResponse", "renderer.applyBatch"}


def read(path, first, last):
    rows = []
    with open(path, errors="replace") as log:
        for index, line in enumerate(log):
            if index < first or index >= last:
                continue
            for m in ROW.finditer(line):
                if m.group(1):
                    rows.append(("P", int(m.group(1)) / 1000, int(m.group(2)) / 1000))
                elif m.group(3):
                    rows.append(("J", int(m.group(3)), float(m.group(4)) * 1000, m.group(5).strip()))
                elif m.group(6):
                    rows.append(("G", int(m.group(7)), int(m.group(9))))
                else:
                    rows.append(("F", int(m.group(10))))
    return rows


def pill(times):
    readings, start, count, last = [], times[0], 0, times[0]
    for now in times[1:]:
        if now - last > 1000:
            start, count = now, 0
        else:
            count += 1
        last = now
        if now - start >= 500:
            readings.append(round(count * 1000 / (now - start)))
            start, count = now, 0
    return sorted(readings)


def main():
    args = [arg for arg in sys.argv[1:] if arg != "-v"]
    verbose = "-v" in sys.argv
    path = args[0]
    first = int(args[1]) if len(args) > 1 else 0
    last = int(args[2]) if len(args) > 2 else 1 << 62
    rows = read(path, first, last)
    presents = [(index, row) for index, row in enumerate(rows) if row[0] == "P"]
    if len(presents) < 2:
        sys.exit("No [gpui-presented] lines; launch with GPUI_WAYLAND_PRESENTATION_TRACE=1.")
    refresh_us = presents[0][1][2]
    readings = pill([row[1] / 1000 for _, row in presents])
    if readings:
        print(f"pill readings: n={len(readings)} min={readings[0]} p10={readings[len(readings) // 10]} "
              f"median={readings[len(readings) // 2]} lowest={readings[:8]}")

    # The first wake after each vblank, minus a small delivery latency.
    offsets = []
    for position, (index, row) in enumerate(presents[:-1]):
        for later in rows[index + 1:presents[position + 1][0]]:
            if later[0] == "J" and later[3].startswith("display.wake"):
                offsets.append(later[1] - row[1])
                break
    offsets.sort()
    offset = offsets[len(offsets) // 20] - 100 if offsets else None

    categories, missed = collections.Counter(), 0
    for (start, before), (end, after) in zip(presents, presents[1:]):
        gap = round((after[1] - before[1]) / refresh_us)
        if gap < 2 or gap > 40:  # >40 vblanks: the window was idle or hidden
            continue
        missed += gap - 1
        between = rows[start + 1:end]
        wakes = [row for row in between if row[0] == "J" and row[3].startswith("display.wake")]
        js = [row for row in between if row[0] == "J" and not row[3].startswith(("display.", "input."))]
        longest = max(js, key=lambda row: row[2]) if js else None
        first_wake = (wakes[0][1] - offset - before[1]) / 1000 if wakes and offset is not None else None
        gpu = [row for row in between if row[0] == "G"]
        ui = [row[1] for row in between if row[0] == "F"]
        name = longest[3].split(" ")[0] if longest else ""
        if offset is None:
            category = "untraced-js"
        elif first_wake is None or first_wake > refresh_us / 1000 * 1.5:
            category = "wake-late"
        elif longest and longest[2] > 4000 and name in SMALL_OPS:
            category = "gc-suspect"
        elif longest and longest[2] > refresh_us * 0.6:
            category = f"js-long:{name}"
        elif longest and first_wake + longest[2] / 1000 > refresh_us / 1000 * 0.85:
            category = "js-late"
        elif max((row[1] for row in gpu), default=0) > 4000 or max((row[2] for row in gpu), default=0) > 4000:
            category = "gpu-block"
        elif ui and max(ui) > 7000:
            category = "ui-long"
        else:
            category = "other"
        categories[category] += gap - 1
        if verbose:
            print(f"gap={gap} first_wake_ms={first_wake if first_wake is None else round(first_wake, 2)} "
                  f"longest_js={longest and (longest[3][:40], round(longest[2] / 1000, 2))} ui_us={ui[:4]} -> {category}")
    span = (presents[-1][1][1] - presents[0][1][1]) / 1e6
    print(f"span={span:.1f}s missed vblanks={missed} ({100 * missed / max(span * 1e6 / refresh_us, 1):.2f}%) "
          f"{dict(categories.most_common())}")


if __name__ == "__main__":
    main()
