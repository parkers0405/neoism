#!/usr/bin/env python3
"""Read Wayland presentation feedback from a Rune WAYLAND_DEBUG=1 capture.

Use during a continuously animating workload. Idle windows intentionally skip frames;
intervals spanning idle time are not evidence of a rendering stall. Unlike the encoded
frame callback, the protocol's timestamp describes actual compositor presentation.
"""
import argparse
import json
import re
import statistics

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('trace')
parser.add_argument('--trim-seconds', type=float, default=0.5)
parser.add_argument('--end-seconds', type=float, help='End of the active workload, relative to the first presentation')
args = parser.parse_args()
pattern = re.compile(r'wp_presentation_feedback#\d+\.presented\((\d+), (\d+), (\d+), (\d+),')
frames = []
for line in open(args.trace):
    match = pattern.search(line)
    if match:
        high, low, nanos, refresh = map(int, match.groups())
        frames.append((((high << 32) + low) * 1_000_000_000 + nanos, refresh))
if len(frames) < 2:
    raise SystemExit('No presentation interval found; encoded-frame counters are not a substitute.')
start = frames[0][0] + args.trim_seconds * 1_000_000_000
end = frames[0][0] + args.end_seconds * 1_000_000_000 if args.end_seconds is not None else float("inf")
frames = [f for f in frames if start <= f[0] <= end]
intervals = [(b[0] - a[0], b[1] or a[1]) for a, b in zip(frames, frames[1:])]
if not intervals or any(period <= 0 for _, period in intervals):
    raise SystemExit('No usable refresh intervals after trimming.')
missed = sum(max(0, round(delta / period) - 1) for delta, period in intervals)
ms = sorted(delta / 1_000_000 for delta, _ in intervals)
result = {
    'presentations': len(frames),
    'seconds': (frames[-1][0] - frames[0][0]) / 1_000_000_000,
    'refresh_hz': 1_000_000_000 / statistics.median(period for _, period in intervals),
    'missed_vblanks': missed,
    'missed_percent': 100 * missed / (len(intervals) + missed),
    'interval_ms': {'p50': statistics.median(ms), 'p95': ms[int(.95 * (len(ms) - 1))], 'p99': ms[int(.99 * (len(ms) - 1))], 'max': max(ms)},
    'assumption': 'Continuously animating workload; includes compositor and system delays.',
}
print(json.dumps(result, indent=2))
