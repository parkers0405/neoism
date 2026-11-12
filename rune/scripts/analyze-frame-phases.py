#!/usr/bin/env python3
"""Summarize a native NEOISM_RUNE_FRAME_TRACE log from a frame-profile build.

Pairs the poll-to-encode interval with Rune's CPU and wall phase timings.
Total wall/CPU cover poll start to callback entry (after present), not just encode.
Commit includes view/layout/text/apply; do not add these overlapping phases.
The residual CPU includes runtime work outside those phases and GPU submission.
Wall minus thread CPU includes both blocking and scheduler preemption, not just GPU waits.
"""
import argparse
import json
import re
from collections import Counter
from pathlib import Path

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('log', type=Path)
p.add_argument('--slowest', type=int, default=10)
a = p.parse_args()

frames, phases, marks = {}, {}, []
pending_events = Counter()
message_counts = {}
frame = re.compile(r'\[rune-frame\] seq=(\d+) at_us=\d+ mono_us=(-?\d+) poll_us=(\d+) work_us=(\d+) rerun=(\w+)')
mark = re.compile(r'\[drive\] mark:(\S+) mono_us=(-?\d+) messages=(\d+)')
event = re.compile(r'\[daemon\] AgentReply (\w+) ')
for line in a.log.read_text(errors='replace').splitlines():
    if m := frame.match(line):
        seq, mono, poll, work = map(int, m.groups()[:4])
        frames[seq] = dict(seq=seq, mono=mono, poll_ms=poll / 1000, work_ms=work / 1000, poll_rerun=m[5] == 'true')
        frames[seq]['events'] = dict(pending_events)
        pending_events.clear()
    elif line.startswith('[rune-phases] '):
        row = json.loads(line[len('[rune-phases] '):])
        phases[row['seq']] = row
    elif m := mark.match(line):
        marks.append((m[1], int(m[2])))
        message_counts[int(m[2])] = int(m[3])
    elif m := event.match(line):
        pending_events[m[1]] += 1

rows = []
for seq, f in frames.items():
    phase = phases.get(seq)
    if not phase or phase['cpu_ms'] is None or phase.get('wall_ms') is None or f['work_ms'] <= 0:
        continue
    f.update(phase)
    f['off_cpu_ms'] = max(0, f['wall_ms'] - f['cpu_ms'])
    f['other_cpu_ms'] = max(0, f['cpu_ms'] - f['commit_cpu_ms'] - f['paint_cpu_ms'])
    rows.append(f)

if not rows:
    raise SystemExit('No paired phase samples. Build with --features frame-profile and enable NEOISM_RUNE_FRAME_TRACE=1.')

def percentile(values, q):
    values = sorted(values)
    return values[int(q * (len(values) - 1))]

for (name, start), (_, end) in zip(marks, marks[1:]):
    span = [r for r in rows if start <= r['mono'] < end]
    if not span:
        continue
    print(f'{name}: {len(span)} samples, {sum(r["poll_rerun"] for r in span)} poll rebuild requests, '
          f'{sum(r["commit_ms"] > 0 for r in span)} frames with commits')
    print(f'  loaded messages: {message_counts[start]} -> {message_counts[end]}')
    events = Counter()
    for r in span:
        events.update(r['events'])
    # Native event trace lines have no clock; associate them with the next encoded frame.
    print('  traced agent events: ' + (', '.join(f'{k}={n}' for k, n in events.most_common()) or 'none (enable --trace-events to observe delivery)'))
    for key in ['work_ms', 'poll_ms', 'wall_ms', 'cpu_ms', 'off_cpu_ms', 'commit_ms', 'paint_ms', 'other_cpu_ms', 'encode_ms', 'after_encode_ms']:
        span_values = [r[key] for r in span if r[key] is not None]
        if not span_values:
            continue
        values = span_values
        print(f'  {key:>14}: p50 {percentile(values, .5):6.2f} p95 {percentile(values, .95):6.2f} max {max(values):6.2f}')

print('\nSlowest marked frames (ms):')
marked = [r for r in rows if marks and marks[0][1] <= r['mono'] < marks[-1][1]]
for r in sorted(marked, key=lambda r: r['work_ms'], reverse=True)[:a.slowest]:
    name = next(name for (name, start), (_, end) in zip(marks, marks[1:]) if start <= r['mono'] < end)
    print(f'  {r["seq"]:5d} {name}: ' + ' '.join(f'{k}={r[k]:.2f}' for k in
          ['work_ms', 'cpu_ms', 'off_cpu_ms', 'commit_ms', 'layout_ms', 'text_ms', 'paint_ms', 'other_cpu_ms']))