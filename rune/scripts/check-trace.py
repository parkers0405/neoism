#!/usr/bin/env python3
"""Assert cursor clipping in Rune's per-frame element trace, native or headless."""
import argparse
import json
import statistics

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('trace')
parser.add_argument('--require-cursor', action='store_true')
parser.add_argument('--require-scroll', action='store_true')
args = parser.parse_args()
scroll_distance = 0.0
metadata, state = {}, {}
frames = cursors = 0
cpu = []
last_cursor = None
for frame in map(json.loads, open(args.trace)):
    if 't' not in frame:
        continue
    frames += 1
    for entry in frame.get('sc', []):
        scroll_distance = max(scroll_distance, abs(entry[2]))
    for entry in frame.get('m', []):
        metadata[entry['id']] = entry['nm']
    for entry in frame.get('e', []):
        state[entry[0]] = entry
    for removed in frame.get('g', []):
        state.pop(removed, None)
    named = {metadata[i].rsplit('/', 1)[-1]: value for i, value in state.items() if i in metadata}
    cursor, clip = named.get('window-cursor'), named.get('window-cursor-clip')
    if cursor and clip:
        cursors += 1
        last_cursor = cursor[1:5]
        explicit_visible = len(cursor) >= 12
        x, y, w, h = cursor[8:12] if explicit_visible else cursor[1:5]
        # Rune snaps clip edges to pixels and omits visible bounds when the
        # difference is <= .5 pt. Account for both without treating subpixel
        # metadata loss as a painted cursor escaping its clip.
        tolerance = .51 if explicit_visible else 1.01
        if w > 0 and h > 0:
            assert x >= clip[1] - tolerance and y >= clip[2] - tolerance, f'Cursor crosses clip start at {frame["t"]} ms'
            assert x + w <= clip[1] + clip[3] + tolerance and y + h <= clip[2] + clip[4] + tolerance, f'Cursor crosses clip end at {frame["t"]} ms'
    if 'cpu_ms' in frame.get('quality', {}):
        cpu.append(frame['quality']['cpu_ms'])
assert frames, 'Trace has no frames'
if args.require_cursor:
    assert cursors, 'Trace never contained a clipped cursor'
if args.require_scroll:
    assert scroll_distance > 100, 'Trace did not scroll at least 100 points'
cpu.sort()
print(json.dumps({'frames': frames, 'cursor_frames': cursors, 'scroll_distance': scroll_distance, 'last_cursor_rect': last_cursor,
    'cpu_ms': {'p50': statistics.median(cpu), 'p95': cpu[int(.95 * (len(cpu) - 1))], 'max': max(cpu)} if cpu else None,
    'clipping': 'passed', 'timing': 'CPU submission only; use analyze-presentations.py for display timing'}, indent=2))
