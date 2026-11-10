#!/usr/bin/env python3
"""Frame pacing of a real Rune window against the running daemon, without taking focus.

The window opens on a headless Hyprland output and runs a `NEOISM_RUNE_DRIVE` script on
itself (`rune/app/src/drive.rs`): real history from the daemon, real wheel/trackpad paths,
real prompts if the script sends one. Nothing is typed or scrolled through the compositor,
so the person at the machine keeps their keyboard, pointer and workspace.

Reported per `mark:` span, from the app's frame trace and Wayland presentation feedback:
UI-thread work per frame (poll start to encode end), how many frames re-ran the views, and
missed refreshes while frames were flowing. Work per frame is the number to trust: a
headless output forgives late frames a panel would drop. On this machine's 144 Hz panel,
frames under 2 ms were never late and 3 ms frames were late about one time in five.

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
p.add_argument('--mode', default='2560x1600@144')
p.add_argument('--scale', type=float, default=2)
p.add_argument('--timeout', type=int, default=90)
p.add_argument('--span', type=float, default=0, help='Also split each span into pieces this many seconds long')
a = p.parse_args()

def hypr(*args): return subprocess.run(['hyprctl', *args], check=True, capture_output=True, text=True).stdout
def monitors(): return {m['name']: m for m in json.loads(hypr('-j', 'monitors'))}
def geometry(m): return (m['x'], m['y'], m['width'], m['height'], m['scale'], round(m['refreshRate']))

before = monitors()
created = a.output not in before
if created:
    hypr('output', 'create', 'headless', a.output)
    # The desktop shell starts a bar on a new output; measured too soon, its startup reads as
    # the app's own slow frames (4 ms where a settled output measures 0.5).
    time.sleep(10)
try:
    out = monitors()[a.output]
    # Where Hyprland put it: giving it a new position re-arranges the other outputs.
    hypr('eval', f'hl.monitor({{ output = "{a.output}", mode = "{a.mode}", position = "{out["x"]}x{out["y"]}", scale = {a.scale} }})')
    time.sleep(.5)
    now = monitors()
    moved = [name for name, m in before.items() if name != a.output and geometry(now[name]) != geometry(m)]
    if moved: raise SystemExit(f'adding {a.output} changed {moved}; restore them before testing')
    workspace = now[a.output]['activeWorkspace']['id']

    base = Path(a.out).absolute(); base.parent.mkdir(parents=True, exist_ok=True)
    log, state = base.with_suffix('.log'), base.with_suffix('.state.json')
    app_id = f'ai.neoism.rune.pacing{os.getpid()}'
    state.write_text(json.dumps({'version': 1, 'active': 0, 'workspaces': [{'name': Path(a.cwd).name, 'cwd': a.cwd,
        'tabs': [{'kind': 'home', 'draft': ''}, {'kind': 'agent', 'session': a.session, 'draft': ''}],
        'layout': {'kind': 'pane', 'tabs': [0, 1], 'active': 1, 'focused': True}, 'active': 1, 'sidebar': None, 'unsaved': {}}]}))
    env = [f'NEOISM_RUNE_STATE_PATH={state}', f'NEOISM_RUNE_APP_ID={app_id}', 'NEOISM_RUNE_FRAME_TRACE=1', 'WAYLAND_DEBUG=1', f'NEOISM_RUNE_DRIVE={a.drive}']
    command = f'cd {shlex.quote(a.cwd)} && ' + shlex.join(['env', *env, 'timeout', '--signal=INT', str(a.timeout), a.binary]) + f' > {shlex.quote(str(log))} 2>&1'
    hypr('dispatch', f'hl.dsp.exec_cmd({json.dumps(command)}, {{ workspace = "{workspace} silent" }})')
    pid = None
    for _ in range(100):
        pid = next((c['pid'] for c in json.loads(hypr('-j', 'clients')) if c.get('class') == app_id), None)
        if pid: break
        time.sleep(.1)
    if pid is None: raise SystemExit(f'the test window never appeared; see {log}')
    deadline = time.monotonic() + a.timeout + 5
    while Path(f'/proc/{pid}').exists() and time.monotonic() < deadline: time.sleep(.2)
finally:
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
    gaps = [b - a for a, b in zip(*(lambda s: (s, s[1:]))([t for t in shown if start <= t <= end])) if b - a < 3.5 * refresh]
    missed = sum(max(0, round(gap / refresh) - 1) for gap in gaps)
    print(f'{name:>18}: {len(span):5d} frames, {sum(f[2] for f in span):4d} view re-runs | work ms p50 {percentile(work, .5):.2f} p95 {percentile(work, .95):.2f} '
          f'p99 {percentile(work, .99):.2f} max {max(work, default=0):.2f} | missed {missed}/{len(gaps) + missed} ({100 * missed / max(1, len(gaps) + missed):.1f}%)'
          f' at {len(gaps) / max(1e-9, sum(gaps)):.1f} fps')

print(f'{len(frames)} frames, {len(shown)} presentations; log {log}')
for (name, start), (_, end) in zip(marks, marks[1:]):
    report(name, start, end)
    t = start
    while a.span and t < end: report(f'{name}+{t - start:.0f}s', t, min(t + a.span, end)); t += a.span
