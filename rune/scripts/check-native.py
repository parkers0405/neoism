#!/usr/bin/env python3
"""Capture a real Rune window on a temporary Hyprland workspace (Lua dispatcher).

The existing preview stays open. Each input batch verifies the test window still
owns focus. Rune traces element geometry; WAYLAND_DEBUG records presentations.
"""
import argparse
import json
import os
from pathlib import Path
import shlex
import subprocess
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--binary', default='/tmp/neoism-rune-target/release/neoism-rune')
parser.add_argument('--output', default='/tmp/neoism-rune-verify/native-editor')
parser.add_argument('--scene', default='editor-long')
parser.add_argument('--workspace', type=int, default=5)
parser.add_argument('--size', help='Resize only the test window, for example 624x674')
parser.add_argument('--frames', type=int, default=720)
parser.add_argument('--natural', action='store_true', help='Use normal redraw scheduling instead of forcing every frame')
parser.add_argument('--keys', default='j' * 140)
parser.add_argument('--wheel', action='store_true', help='Scroll up and down over the test window using /dev/uinput')
parser.add_argument('--wheel-seconds', type=float, default=3, help='Duration of alternating native wheel input')
parser.add_argument('--daemon-url', help='Use a scratch daemon instead of offline fixtures')
parser.add_argument('--terminal', action='store_true', help='Run bounded ANSI output in a scratch daemon terminal')
args = parser.parse_args()
if args.terminal and not args.daemon_url:
    parser.error('--terminal requires --daemon-url pointing at a scratch daemon')
out = Path(args.output).absolute()
out.parent.mkdir(parents=True, exist_ok=True)
trace = out.with_suffix('.jsonl')
log = out.with_suffix('.wayland.log')

def active():
    return json.loads(subprocess.check_output(['hyprctl', '-j', 'activewindow']))

def dispatch(value):
    subprocess.run(['hyprctl', 'dispatch', value], check=True, capture_output=True)

previous = active()
environment = ['WAYLAND_DEBUG=1']
if args.daemon_url:
    environment.append('NEOISM_GPUIX_DAEMON_URL=' + args.daemon_url)
else:
    environment.append('NEOISM_RUNE_OFFLINE=1')
command = shlex.join(['env', *environment, 'timeout', '--signal=INT', str(max(12, args.wheel_seconds + 5)), args.binary,
    '--scene', args.scene, '--native', '--trace', str(trace), *([] if args.natural else ['--frames', str(args.frames)])])
command += ' > ' + shlex.quote(str(log)) + ' 2>&1'
dispatch(f'hl.dsp.exec_cmd({json.dumps(command)}, {{ workspace = "{args.workspace}" }})')
window = None
for _ in range(80):
    candidate = active()
    try:
        cmd = Path(f'/proc/{candidate["pid"]}/cmdline').read_bytes()
    except (KeyError, FileNotFoundError):
        cmd = b''
    if str(trace).encode() in cmd:
        window = candidate
        break
    time.sleep(.1)
if window is None:
    raise SystemExit('Test window did not focus; no input sent')

if args.size:
    width, height = map(int, args.size.split('x'))
    target = json.dumps('address:' + window['address'])
    dispatch(f'hl.dsp.window.float({{ action = "on", window = {target} }})')
    dispatch(f'hl.dsp.window.resize({{ x = {width}, y = {height}, relative = false, window = {target} }})')
    time.sleep(.5)
    window = next(w for w in json.loads(subprocess.check_output(['hyprctl', '-j', 'clients'])) if w['pid'] == window['pid'])

def send(*keys):
    if active().get('pid') != window['pid']:
        raise SystemExit('Focus changed; input stopped')
    subprocess.run(['wtype', *keys], check=True)

time.sleep(.6)
if args.terminal:
    send('-M', 'alt', '-k', 't', '-m', 'alt')
    time.sleep(.5)
    workload = out.with_suffix('.workload.py')
    marker = out.with_suffix('.workload-complete')
    marker.unlink(missing_ok=True)
    workload.write_text("import sys,time,pathlib\n"
        "sys.stdout.write('\\033[?1049h\\033[?25l')\n"
        "try:\n"
        " for frame in range(360):\n"
        "  sys.stdout.write('\\033[H' + ''.join('\\033[32mRune TUI frame %d row %d\\033[0m\\033[K\\r\\n' % (frame,row) for row in range(24)))\n"
        "  sys.stdout.flush(); time.sleep(.008)\n"
        "finally:\n"
        " sys.stdout.write('\\033[?25h\\033[?1049l'); sys.stdout.flush()\n"
        + f"pathlib.Path({str(marker)!r}).write_text('360 frames completed')\n")
    send(shlex.join(['python3', str(workload)]))
    send('-k', 'Return')
elif args.wheel:
    import fcntl
    import struct
    dispatch(f'hl.dsp.cursor.move({{ x = {window["at"][0] + window["size"][0] * .7}, y = {window["at"][1] + window["size"][1] * .4} }})')
    fd = os.open('/dev/uinput', os.O_WRONLY | os.O_NONBLOCK)
    created = False
    try:
        fcntl.ioctl(fd, 0x40045564, 1)
        fcntl.ioctl(fd, 0x40045565, 0x110)
        fcntl.ioctl(fd, 0x40045564, 2)
        for relative in (0, 1, 8, 11):
            fcntl.ioctl(fd, 0x40045566, relative)
        fcntl.ioctl(fd, 0x405c5503, struct.pack('HHHH80sI', 3, 0x1234, 0x5678, 1, b'neoism-rune-test-wheel', 0))
        fcntl.ioctl(fd, 0x5501)
        created = True
        time.sleep(.5)
        wheel_end = time.monotonic() + args.wheel_seconds
        direction = -1
        while time.monotonic() < wheel_end:
            start = time.monotonic()
            for notch in range(45):
                if time.monotonic() >= wheel_end: break
                if active().get('pid') != window['pid']:
                    raise SystemExit('Focus changed; wheel input stopped')
                time.sleep(max(0, start + notch / 30 - time.monotonic()))
                for kind, code, value in [(2, 8, direction), (2, 11, 120 * direction), (0, 0, 0)]:
                    os.write(fd, struct.pack('llHHi', 0, 0, kind, code, value))
            direction = -direction
    finally:
        if created:
            fcntl.ioctl(fd, 0x5502)
        os.close(fd)
else:
    for offset in range(0, len(args.keys), 5):
        send('-d', '18', args.keys[offset:offset + 5])
deadline = time.monotonic() + 15
while Path(f'/proc/{window["pid"]}').exists() and time.monotonic() < deadline:
    time.sleep(.1)
current = active()
if previous and (not current or current.get('workspace', {}).get('id') == args.workspace):
    dispatch(f'hl.dsp.focus({{ window = "address:{previous["address"]}" }})')
if args.terminal:
    assert marker.exists(), 'TUI workload did not complete; capture is not a valid terminal benchmark'
print(json.dumps({'pid': window['pid'], 'size': window['size'], 'trace': str(trace), 'presentations': str(log)}))
