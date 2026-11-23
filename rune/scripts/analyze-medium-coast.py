"""Measure placement gaps, not just drawable cadence, in the fixed native hand trace."""
import re
import sys
from pathlib import Path
from statistics import median

present = []
coasts = []
frames = None
for line in Path(sys.argv[1]).read_text().splitlines():
    if line == '[medium] coast begin':
        frames, moved = [], None
    elif line == '[medium] coast end':
        coasts.append(frames)
        frames = None
    elif m := re.match(r'\[rune-present\] mono_us=(\d+)', line):
        present.append(int(m[1]))
    elif frames is not None:
        if m := re.match(r'\[scroll\] conversation:\S+ .*offset=([\d.]+) max=([\d.]+) to=Some\(([\d.]+)\)', line):
            moved = float(m[3])
            if not 0 < min(float(m[1]), moved) <= max(float(m[1]), moved) < float(m[2]):
                sys.exit('edge in coast: reposition before measuring')
        elif m := re.match(r'\[rune-frame\] .*?mono_us=(\d+) .*outcome=(\w+)', line):
            frames.append((int(m[1]), moved, m[2]))
            moved = None

if len(coasts) != 2:
    sys.exit('expected two complete marked coasts')
for direction, frames in zip(('older', 'newer'), coasts):
    if not frames or frames[-1][0] - frames[0][0] < 1_200_000:
        sys.exit('short coast')
    # Anchor to the last completed frame BEFORE momentum-end (within one refresh).
    end = frames[-1][0]
    window = [f for f in frames if f[0] > end - 500_000]
    moves = [(t, pos) for t, pos, outcome in window if pos is not None]
    if len(moves) < 2 or any(f[2] != 'Drawn' for f in window):
        sys.exit('insufficient motion or covered window')
    def gaps(times):
        return [(b-a)/1000 for a, b in zip(times, times[1:])]
    mg = gaps([t for t, _ in moves])
    pg = gaps(sorted(t for t in present if end - 500_000 < t <= end))
    if not pg or not 7.5 < median(pg) < 9:
        sys.exit('no 120 Hz presentation evidence')
    print(f'{direction}: {len(moves)}/{len(window)} frames placed, range={min(p for _,p in moves):.2f}..{max(p for _,p in moves):.2f}pt; '
          f'{sum(g >= 12.5 for g in mg)} placement gaps >=12.5ms, max={max(mg):.2f}ms; '
          f'presentation p50/max={median(pg):.2f}/{max(pg):.2f}ms')