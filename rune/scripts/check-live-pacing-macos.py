#!/usr/bin/env python3
"""Frame pacing of a real Rune window on macOS, against the running daemon.

The macOS counterpart of `check-live-pacing.py` (which needs Hyprland). The window runs a
`NEOISM_RUNE_DRIVE` script on itself (`rune/app/src/drive.rs`) and the app's frame trace
(`NEOISM_RUNE_FRAME_TRACE=1`) writes, on one clock:

  [rune-frame]    each submitted frame: UI-thread work, whether the views re-ran
  [rune-present]  when each drawable reached the screen (Metal `presentedTime`)
  [drive]         each step of the script, including `mark:<name>`

Reported per `mark:` span: presentations per second, missed refreshes (a presentation
interval of two or more refresh periods), the worst intervals, and UI-thread work per frame.
The window must be visible on a 120 Hz display: macOS does not present a covered window.

    rune/app/build.sh build --release
    python3 rune/scripts/check-live-pacing-macos.py --session <id> \\
        --drive "ready;wait:2;mark:up;wheel:30:3;mark:down;wheel:-30:3;mark:end;quit"
"""
import argparse, json, os, re, subprocess, sys, time
from pathlib import Path
from rune_paths import default_binary

p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
p.add_argument('--binary', default=default_binary())
p.add_argument('--drive', required=True, help='NEOISM_RUNE_DRIVE script; end it with `quit`')
p.add_argument('--session', help='Conversation to open (omit for a new chat)')
p.add_argument('--scene', help='Measure an offline Rune scene instead of a live conversation')
p.add_argument('--cwd', default=os.getcwd(), help='Workspace directory of the test window')
p.add_argument('--out', default='/tmp/neoism-rune-verify/live-pacing')
p.add_argument('--hz', type=float, default=120.0, help='Display refresh rate')
p.add_argument('--timeout', type=int, default=120)
p.add_argument('--size', help='Window size in points, WxH')
p.add_argument('--trace-events', action='store_true', help='Include poll causes and daemon message kinds')
p.add_argument('--env', action='append', default=[], help='Extra KEY=VALUE for the test window')
p.add_argument('--span', type=float, default=0, help='Also split each span into pieces this many seconds long')
p.add_argument('--parse', action='store_true', help='Only re-read an existing <out>.log')
p.add_argument('--require', type=float, help='Fail unless every span misses at most this percent of refreshes')
a = p.parse_args()

base = Path(a.out).absolute(); base.parent.mkdir(parents=True, exist_ok=True)
log, state = base.with_suffix('.log'), base.with_suffix('.state.json')
if not a.parse:
    version = subprocess.run([a.binary, '--version'], check=True, capture_output=True, text=True).stdout.strip()
    print(f'Capturing {version} ({Path(a.binary).resolve()})', flush=True)
    window = {'version': 1, 'active': 0, 'workspaces': [{'name': Path(a.cwd).name, 'cwd': a.cwd,
        'tabs': [{'kind': 'home', 'draft': ''}, {'kind': 'agent', 'session': a.session, 'draft': ''}],
        'layout': {'kind': 'pane', 'tabs': [0, 1], 'active': 1, 'focused': True}, 'active': 1, 'sidebar': None, 'unsaved': {}}]}
    state.write_text(json.dumps(window))
    env = dict(os.environ, NEOISM_RUNE_STATE_PATH=str(state), NEOISM_RUNE_FRAME_TRACE='1', NEOISM_RUNE_DRIVE=a.drive)
    args = [a.binary]
    if a.scene:
        del env['NEOISM_RUNE_STATE_PATH']
        env.update(NEOISM_RUNE_OFFLINE='1', NEOISM_RUNE_NO_RESTORE='1')
        args += ['--scene', a.scene, '--native']
    if a.size: args += ['--size', a.size]
    if a.trace_events: env.update(NEOISM_RUNE_TRACE_POLL='1', NEOISM_RUNE_TRACE_DAEMON='1')
    for item in a.env:
        key, _, value = item.partition('='); env[key] = value
    with log.open('w') as out:
        out.write('[capture] ' + json.dumps({'binary': str(Path(a.binary).resolve()), 'version': version, 'drive': a.drive}) + '\n'); out.flush()
        app = subprocess.Popen(args, cwd=a.cwd, env=env, stdout=out, stderr=subprocess.STDOUT)
        deadline = time.monotonic() + a.timeout
        while app.poll() is None:
            if time.monotonic() > deadline:
                app.terminate(); print('timed out; reporting what was captured'); break
            time.sleep(0.05)

lines = log.read_text(errors='replace').splitlines()
present = re.compile(r'\[rune-present\] mono_us=(-?\d+)')
frame = re.compile(r'\[rune-frame\] seq=\d+ at_us=\d+ mono_us=(-?\d+) poll_us=(\d+) work_us=(\d+) rerun=(\w+) outcome=(\w+)')
step = re.compile(r'\[drive\] (\S+) mono_us=(-?\d+)(.*)')
shown, frames, marks = [], [], []
for line in lines:
    if m := present.match(line): shown.append(int(m[1]) / 1e6)
    elif m := frame.match(line): frames.append((int(m[1]) / 1e6, int(m[3]) / 1000, m[4] == 'true', m[5], int(m[2]) / 1000))
    elif m := step.match(line):
        if m[1].startswith('mark:'): marks.append((m[1][5:], int(m[2]) / 1e6))
        print(f'  {m[1]}{m[3]}')
shown.sort()
refresh = 1 / a.hz

def percentile(values, q): values = sorted(values); return values[int(q * (len(values) - 1))] if values else 0.0
worst_miss = 0.0
def report(name, start, end):
    global worst_miss
    if end - start < refresh: return
    span = [f for f in frames if start <= f[0] <= end]
    work = [f[1] for f in span if f[1] > 0]
    times = [t for t in shown if start <= t <= end]
    gaps = [b - a for a, b in zip(times, times[1:])]
    # A gap of n refresh periods missed n-1 refreshes; a rest (>= 100 ms) is not motion.
    active = [g for g in gaps if g < 0.1]
    rests = [g for g in gaps if g >= 0.1]
    missed = sum(max(0, round(g / refresh) - 1) for g in active)
    expected = sum(max(1, round(g / refresh)) for g in active)
    pct = 100 * missed / expected if expected else 0.0
    worst_miss = max(worst_miss, pct)
    fps = len(active) / sum(active) if active else 0.0
    slow = sorted(((g * 1000, t - start) for g, t in zip(gaps, times[1:]) if 1.5 * refresh <= g < 0.1), reverse=True)[:6]
    reruns = sum(1 for f in span if f[2])
    covered = sum(1 for f in span if f[3] == 'Skipped')
    # What the status line's FPS badge reads (`ui/fps.rs`): intervals completed in the second
    # before the newest presentation, sampled twice a second while frames arrive.
    badge, at = [], start + 1.0
    while at <= end:
        upto = [t for t in shown if t <= at]
        if upto and at - upto[-1] < 0.5:
            last = upto[-1]; first = max((i for i, t in enumerate(upto) if t <= last - 1.0), default=0)
            if last - upto[first] >= 0.25: badge.append(round((len(upto) - first - 1) / (last - upto[first])))
        at += 0.5
    print(f'{name:>10}: {end - start:6.2f}s shown={len(times):5d} fps={fps:6.1f} missed={missed:4d} ({pct:5.2f}%) rests={len(rests)} '
          f'gap p50={percentile(active, .5) * 1000:5.2f} p99={percentile(active, .99) * 1000:5.2f} max={max(active, default=0) * 1000:6.2f}ms | '
          f'frames={len(span)} reruns={reruns} work p50={percentile(work, .5):5.2f} p95={percentile(work, .95):5.2f} p99={percentile(work, .99):5.2f} max={max(work, default=0):6.2f}ms')
    if covered: print(' ' * 12 + f'COVERED: {covered} frames were skipped because the window was hidden or covered; rerun with it visible')
    if badge: print(' ' * 12 + f'badge: min={min(badge)} max={max(badge)} readings={" ".join(map(str, badge))}')
    if slow: print(' ' * 12 + 'late: ' + ', '.join(f'{g:.1f}ms@{at:.2f}s' for g, at in slow))
    heavy = sorted(((f[1], f[0] - start, f[2], f[4]) for f in span if f[1] > 4.0), reverse=True)[:6]
    if heavy: print(' ' * 12 + 'heavy: ' + ', '.join(f'{w:.1f}ms(poll {pl:.1f}{" rerun" if r else ""})@{at:.2f}s' for w, at, r, pl in heavy))

print(f'{len(shown)} presentations, {len(frames)} frames, refresh {a.hz:g} Hz')
for (name, start), (_, end) in zip(marks, marks[1:]):
    report(name, start, end)
    if a.span:
        t = start
        while t + a.span <= end + 1e-6:
            report(f'+{t - start:.0f}s', t, min(t + a.span, end)); t += a.span
if a.require is not None and worst_miss > a.require:
    sys.exit(f'FAILED: a span missed {worst_miss:.2f}% of refreshes (allowed {a.require}%)')
