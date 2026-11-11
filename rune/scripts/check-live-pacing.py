#!/usr/bin/env python3
"""Frame pacing of a real Rune window against the running daemon.

By default the window opens on a headless Hyprland output and runs a `NEOISM_RUNE_DRIVE` script on
itself (`rune/app/src/drive.rs`): real history from the daemon, real wheel/trackpad paths,
real prompts if the script sends one. Nothing is typed or scrolled through the compositor,
so the person at the machine keeps their keyboard, pointer and workspace.
Use --physical for actual panel presentation timing on a temporary visible workspace;
it restores the previous window afterward unless the user switched away. Existing
display modes and scales are never changed.

Reported per `mark:` span, from the app's frame trace and Wayland presentation feedback:
UI-thread work per frame (poll start to encode end), how many frames re-ran the views, and
missed refreshes while frames were flowing, plus longer gaps (idle time or stalls).
Use --physical to assess missed refreshes: a headless output can forgive late frames
a physical panel would drop. Longer gaps need input/motion context to distinguish
intentional rest from a stall.

    rune/app/build.sh build --release
    python3 rune/scripts/check-live-pacing.py --session <id> \\
        --drive "ready;wait:2;mark:up;wheel:30:3;mark:down;wheel:-30:3;mark:end;quit"
    python3 rune/scripts/check-live-pacing.py --session <id> --drive \\
        "ready;wait:2;live;wait:1;mark:stream;stream:/path/reply.md:10;mark:end;quit"
"""
import argparse, json, os, re, shlex, subprocess, time
from pathlib import Path

p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
p.add_argument('--binary', default='/tmp/neoism-rune-target/release/neoism-rune')
p.add_argument('--drive', required=True, help='NEOISM_RUNE_DRIVE script; end it with `quit`')
p.add_argument('--session', help='Conversation to open (omit for a new chat)')
p.add_argument('--cwd', default=os.getcwd(), help='Workspace directory of the test window')
p.add_argument('--out', default='/tmp/neoism-rune-verify/live-pacing')
p.add_argument('--output', default='neoperf', help='Headless output to use (created and removed if missing)')
p.add_argument('--physical', action='store_true', help='Use a temporary workspace on the active display, then restore focus')
p.add_argument('--mode', help='Require this existing output mode; never reconfigure other displays')
p.add_argument('--timeout', type=int, default=90)
p.add_argument('--trace-events', action='store_true', help='Include poll causes and daemon message kinds (no message bodies)')
p.add_argument('--span', type=float, default=0, help='Also split each span into pieces this many seconds long')
a = p.parse_args()

def hypr(*args): return subprocess.run(['hyprctl', *args], check=True, capture_output=True, text=True).stdout
def monitors(): return {m['name']: m for m in json.loads(hypr('-j', 'monitors'))}
def geometry(m): return (m['x'], m['y'], m['width'], m['height'], m['scale'], round(m['refreshRate']))

workspace = None
before = monitors()
previous = json.loads(hypr('-j', 'activewindow')) if a.physical else None
created = not a.physical and a.output not in before
if created:
    hypr('output', 'create', 'headless', a.output)
    # The desktop shell starts a bar on a new output; measured too soon, its startup reads as
    # the app's own slow frames (4 ms where a settled output measures 0.5).
    time.sleep(10)
try:
    now = monitors()
    moved = [name for name, m in before.items() if name != a.output and geometry(now[name]) != geometry(m)]
    if moved: raise SystemExit(f'adding {a.output} changed {moved}; restore them before testing')
    out = next(m for m in now.values() if m.get('focused')) if a.physical else now[a.output]
    actual_mode = f'{out["width"]}x{out["height"]}@{round(out["refreshRate"])}'
    if a.mode and a.mode != actual_mode: raise SystemExit(f'output is {actual_mode}, requested {a.mode}; no display settings changed')
    a.mode = actual_mode
    if a.physical:
        occupied = {w['id'] for w in json.loads(hypr('-j', 'workspaces'))}
        workspace = next(i for i in range(90, 1000) if i not in occupied)
    else: workspace = out['activeWorkspace']['id']

    base = Path(a.out).absolute(); base.parent.mkdir(parents=True, exist_ok=True)
    log, state = base.with_suffix('.log'), base.with_suffix('.state.json')
    app_id = f'ai.neoism.rune.pacing{os.getpid()}'
    state.write_text(json.dumps({'version': 1, 'active': 0, 'workspaces': [{'name': Path(a.cwd).name, 'cwd': a.cwd,
        'tabs': [{'kind': 'home', 'draft': ''}, {'kind': 'agent', 'session': a.session, 'draft': ''}],
        'layout': {'kind': 'pane', 'tabs': [0, 1], 'active': 1, 'focused': True}, 'active': 1, 'sidebar': None, 'unsaved': {}}]}))
    env = [f'NEOISM_RUNE_STATE_PATH={state}', f'NEOISM_RUNE_APP_ID={app_id}', 'NEOISM_RUNE_FRAME_TRACE=1', 'WAYLAND_DEBUG=1', f'NEOISM_RUNE_DRIVE={a.drive}']
    if a.trace_events: env += ['NEOISM_RUNE_TRACE_POLL=1', 'NEOISM_RUNE_TRACE_DAEMON=1']
    command = f'cd {shlex.quote(a.cwd)} && ' + shlex.join(['env', *env, 'timeout', '--signal=INT', str(a.timeout), a.binary]) + f' > {shlex.quote(str(log))} 2>&1'
    destination = str(workspace) + ('' if a.physical else ' silent')
    hypr('dispatch', f'hl.dsp.exec_cmd({json.dumps(command)}, {{ workspace = {json.dumps(destination)} }})')
    pid = None
    for _ in range(100):
        pid = next((c['pid'] for c in json.loads(hypr('-j', 'clients')) if c.get('class') == app_id), None)
        if pid: break
        time.sleep(.1)
    if pid is None: raise SystemExit(f'the test window never appeared; see {log}')
    deadline = time.monotonic() + a.timeout + 5
    while Path(f'/proc/{pid}').exists() and time.monotonic() < deadline: time.sleep(.2)
finally:
    if a.physical and workspace is not None and previous.get('address'):
        current = json.loads(hypr('-j', 'activewindow'))
        if not current or current.get('workspace', {}).get('id') == workspace:
            hypr('dispatch', 'hl.dsp.focus({window=' + json.dumps('address:' + previous['address']) + '})')
    if created: hypr('output', 'remove', a.output)

lines = log.read_text(errors='replace').splitlines()
presented = re.compile(r'wp_presentation_feedback#\d+\.presented\((\d+), (\d+), (\d+), (\d+),')
frame = re.compile(r'\[rune-frame\] seq=\d+ at_us=\d+ mono_us=(-?\d+) poll_us=(\d+) work_us=(\d+) rerun=(\w+)')
step = re.compile(r'\[drive\] (\S+) mono_us=(-?\d+)')
shown, frames, marks = [], [], []
for line in lines:
    if m := presented.search(line):
        high, low, nanos, _ = map(int, m.groups()); shown.append(((high << 32) + low) + nanos / 1e9)
    elif m := frame.match(line): frames.append((int(m[1]) / 1e6, int(m[3]) / 1000, m[4] == 'true'))
    elif (m := step.match(line)) and m[1].startswith('mark:'): marks.append((m[1][5:], int(m[2]) / 1e6))
refresh = 1 / float(a.mode.split('@')[1])

def percentile(values, q): values = sorted(values); return values[int(q * (len(values) - 1))] if values else 0.0
def report(name, start, end):
    span = [f for f in frames if start <= f[0] <= end]
    if len(span) < 3: return
    work = [f[1] for f in span if f[1] > 0]
    times = [t for t in shown if start <= t <= end]
    all_gaps = [b - a for a, b in zip(times, times[1:])]
    gaps = [gap for gap in all_gaps if gap < 3.5 * refresh]
    long_gaps = [gap for gap in all_gaps if gap >= 3.5 * refresh]
    missed = sum(max(0, round(gap / refresh) - 1) for gap in gaps)
    print(f'{name:>18}: {len(span):5d} frames, {sum(f[2] for f in span):4d} view re-runs | work ms p50 {percentile(work, .5):.2f} p95 {percentile(work, .95):.2f} '
          f'p99 {percentile(work, .99):.2f} max {max(work, default=0):.2f} | missed {missed}/{len(gaps) + missed} ({100 * missed / max(1, len(gaps) + missed):.1f}%)'
          f' at {len(gaps) / max(1e-9, sum(gaps)):.1f} fps | long gaps {len(long_gaps)}, max gap {1000 * max(all_gaps, default=0):.1f} ms')

print(f'{len(frames)} frames, {len(shown)} presentations; log {log}')
for (name, start), (_, end) in zip(marks, marks[1:]):
    report(name, start, end)
    t = start
    while a.span and t < end: report(f'{name}+{t - start:.0f}s', t, min(t + a.span, end)); t += a.span
