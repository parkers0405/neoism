#!/usr/bin/env python3
"""Real native Helium/Chromium test in nested Hyprland, never host injection.
Run: NEOISM_LIVE_KEYBOARD_TEST=1 python3 scripts/check-computer-browser-live.py \
    --nested --render-node /dev/dri/renderD129
Only the compositor sees the host socket/render node. Browser retains its sandbox.
The fixture observes ordinary events and values; it never injects input or edits DOM.
"""
import argparse
import hashlib
import http.server
import json
import os
import pathlib
import shlex
import shutil
import stat
import subprocess
import sys
import tempfile
import threading
import time
import urllib.parse

ROOT = pathlib.Path(__file__).resolve().parents[1]
HTML = b'''<!doctype html><meta charset="utf-8"><title>Neoism browser regression</title>
<style>body{font:24px sans-serif}input,textarea,[contenteditable]{display:block;box-sizing:border-box;width:90%;margin:20px;height:80px;border:2px solid}#input{background:rgb(237,255,211)}#textarea{background:rgb(211,239,255)}#editable{background:rgb(255,219,241)}</style>
<input id="input" autofocus><textarea id="textarea"></textarea><div id="editable" contenteditable="true" tabindex="0"></div>
<script>
const events=[];let seq=0,pointerCount=0,inputCount=0;const generation=crypto.randomUUID();
function report(){const values={},rects={};for(const id of ['input','textarea','editable']){const el=document.getElementById(id);values[id]=id==='editable'?el.innerText:el.value;const r=el.getBoundingClientRect();rects[id]={x:r.x,y:r.y,width:r.width,height:r.height};}
fetch('/report',{method:'POST',body:JSON.stringify({generation,href:location.href,query:location.search,seq:++seq,ready:document.readyState,focused:document.hasFocus(),activeElement:document.activeElement.id,values,editableTextContent:document.getElementById('editable').textContent,editableHTML:document.getElementById('editable').innerHTML,rects,pointerCount,inputCount,viewport:{width:innerWidth,height:innerHeight,dpr:devicePixelRatio},events})}).catch(()=>{});}
for(const type of ['keydown','keyup','beforeinput','input','pointerdown','focusin','focusout'])document.addEventListener(type,e=>{if(type==='pointerdown')pointerCount++;if(type==='input'||type==='beforeinput')inputCount++;events.push({type,key:e.key,code:e.code,data:e.data,inputType:e.inputType,target:e.target.id,x:e.clientX,y:e.clientY});if(events.length>400)events.shift();report();},true);
window.addEventListener('focus',report);window.addEventListener('blur',report);
setInterval(report,100);report();
</script>'''


def client(run, browser):
    runtime = pathlib.Path(os.environ['XDG_RUNTIME_DIR'])
    assert runtime == run / 'r'
    display = pathlib.Path(os.environ['WAYLAND_DISPLAY'])
    assert len(display.parts) == 1 and not display.is_absolute()
    assert stat.S_ISSOCK((runtime / display).stat().st_mode)
    signature = os.environ['HYPRLAND_INSTANCE_SIGNATURE']
    assert signature and '/' not in signature
    assert stat.S_ISSOCK((runtime / 'hypr' / signature / '.socket.sock').stat().st_mode)
    assert not pathlib.Path('/mnt/wayland').exists()
    assert not pathlib.Path('/dev/input').exists()
    assert not list(pathlib.Path('/dev/dri').glob('*'))
    os.environ.pop('DISPLAY', None)
    os.environ.pop('WAYLAND_SOCKET', None)
    print('VERIFIED private endpoints, no host socket/input/DRM:', runtime, display, signature, flush=True)
    requested_scale = float(os.environ['NEOISM_BROWSER_SCALE'])
    monitors = json.loads(subprocess.check_output(['hyprctl', '-i', signature, '-j', 'monitors']))
    assert len(monitors) == 1
    if os.environ.get('NEOISM_BROWSER_HEADLESS_OUTPUT') == '1':
        previous = monitors[0]['name']
        subprocess.run(['hyprctl', '-i', signature, 'output', 'create', 'headless', 'BROWSER-TEST'], check=True)
        subprocess.run(['hyprctl', '-i', signature, 'keyword', 'monitor',
                        f'BROWSER-TEST,1280x800@60,0x0,{requested_scale}'], check=True)
        subprocess.run(['hyprctl', '-i', signature, 'output', 'remove', previous], check=True)
        monitors = json.loads(subprocess.check_output(['hyprctl', '-i', signature, '-j', 'monitors']))
        assert len(monitors) == 1 and monitors[0]['name'] == 'BROWSER-TEST', monitors
        print('Private headless test output inside nested compositor; nested presentation output removed', flush=True)
    # A host-controlled nested window size need not divide evenly by 1.25.
    # Set and verify ONLY the explicit private compositor's output scale.
    subprocess.run(['hyprctl', '-i', signature, 'keyword', 'debug:disable_scale_checks', 'true'], check=True)
    subprocess.run(['hyprctl', '-i', signature, 'keyword', 'monitor',
                    f"{monitors[0]['name']},{monitors[0]['width']}x{monitors[0]['height']}@60,0x0,{requested_scale}"], check=True)
    deadline = time.monotonic() + 8
    while True:
        monitors = json.loads(subprocess.check_output(['hyprctl', '-i', signature, '-j', 'monitors']))
        if len(monitors) == 1 and abs(monitors[0]['scale'] - requested_scale) < .001:
            break
        assert time.monotonic() < deadline, f'Requested scale {requested_scale} not applied: {monitors}'
        time.sleep(.05)
    (run / 'monitor-start.json').write_text(json.dumps(monitors))
    print('VERIFIED actual output scale:', requested_scale, flush=True)
    report_path = run / 'report.json'

    class Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            if self.path.split('?', 1)[0] != '/':
                self.send_error(404)
                return
            self.server.fixture_path = self.path
            with (run / 'navigation.jsonl').open('a') as out:
                out.write(json.dumps({'path': self.path, 'time_ns': time.monotonic_ns()}) + '\n')
            self.send_response(200)
            self.send_header('Content-Type', 'text/html; charset=utf-8')
            self.end_headers()
            self.wfile.write(HTML)
        def do_POST(self):
            body = self.rfile.read(int(self.headers['Content-Length']))
            value = json.loads(body)
            location = urllib.parse.urlsplit(value['href'])
            report_path_query = location.path + ('?' + location.query if location.query else '')
            if report_path_query != self.server.fixture_path:
                self.send_response(204)  # Late old-document report after navigation.
                self.end_headers()
                return
            # Single HTTP server, atomic report replacement for Rust's reader.
            temporary = run / 'report.tmp'
            temporary.write_text(json.dumps(value, ensure_ascii=False))
            temporary.replace(report_path)
            with (run / 'reports.jsonl').open('a') as out:
                out.write(json.dumps(value, ensure_ascii=False) + '\n')
            self.send_response(204)
            self.end_headers()
        def log_message(self, *_):
            pass

    server = http.server.HTTPServer(('127.0.0.1', 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    with (run / 'fcitx.log').open('w') as log:
        ime = subprocess.Popen(['fcitx5'], stdout=log, stderr=subprocess.STDOUT,
                               env=os.environ | {'WAYLAND_DEBUG': 'client'})
        deadline = time.monotonic() + 15
        while True:
            result = subprocess.run(['dbus-send', '--session', '--print-reply', '--dest=org.freedesktop.DBus',
                                     '/org/freedesktop/DBus', 'org.freedesktop.DBus.NameHasOwner', 'string:org.fcitx.Fcitx5'], capture_output=True)
            if b'boolean true' in result.stdout:
                break
            assert ime.poll() is None and time.monotonic() < deadline, 'private Fcitx startup failed'
            time.sleep(.1)
        command = [browser, '--ozone-platform=wayland', '--class=neoism-browser-regression',
                   '--user-data-dir=' + str(run / 'profile'), '--no-first-run', '--no-default-browser-check',
                   '--disable-sync', '--disable-extensions', '--disable-background-networking',
                   '--disable-gpu', '--password-store=basic', '--enable-wayland-ime',
                   '--disable-features=WaylandWpColorManagerV1',
                   f'http://127.0.0.1:{server.server_port}/']
        (run / 'browser-command.txt').write_text(shlex.join(command))
        with (run / 'browser.log').open('w') as browser_log:
            proc = subprocess.Popen(command, stdout=browser_log, stderr=subprocess.STDOUT)
            env = os.environ | {'NEOISM_LIVE_BROWSER_TEST': '1', 'NEOISM_BROWSER_PID': str(proc.pid),
                                'NEOISM_BROWSER_RUNTIME': str(runtime), 'NEOISM_BROWSER_REPORT': str(report_path),
                                'NEOISM_BROWSER_SCREENSHOT': str(run / 'omnibox-key-x.png'),
                                'NEOISM_BROWSER_LAUNCHER': str(pathlib.Path(__file__).resolve()),
                                'NEOISM_BROWSER_RUN': str(run),
                                'NEOISM_BROWSER_URL': f'http://127.0.0.1:{server.server_port}/',
                                'NEOISM_BROWSER_NAVIGATION': str(run / 'navigation.jsonl')}
            try:
                # Browser-local acknowledgement, not a guessed startup delay.
                deadline = time.monotonic() + 35
                while not report_path.exists():
                    assert proc.poll() is None, 'Browser exited: inspect browser.log'
                    assert time.monotonic() < deadline, 'No browser-local HTTP readiness ACK'
                    time.sleep(.05)
                with (run / 'test.log').open('w') as test_log:
                    result = subprocess.run([sys.executable, str(ROOT / 'scripts/check-computer-keymap.py'),
                        '--features', 'browser-live', 'hyprland_production_browser_roundtrip', '--', '--ignored', '--nocapture'],
                        env=env, stdout=test_log, stderr=subprocess.STDOUT, timeout=150)
                (run / 'test.status').write_text(str(result.returncode))
            finally:
                proc.terminate()
                try:
                    proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    proc.kill()
                    proc.wait()
                ime.terminate()
                server.shutdown()


def observe_pointer(run, field):
    """Screenshot pixels select the target; DOM rectangles only check the result."""
    from PIL import Image
    monitors = json.loads(subprocess.check_output(['hyprctl', '-i', os.environ['HYPRLAND_INSTANCE_SIGNATURE'], '-j', 'monitors']))
    assert len(monitors) == 1
    monitor = monitors[0]
    path = run / ('pointer-' + field + '-' + str(time.monotonic_ns()) + '.png')
    subprocess.run(['grim', '-o', monitor['name'], str(path)], check=True)
    image = Image.open(path).convert('RGB')
    color = {'input': (237, 255, 211), 'textarea': (211, 239, 255), 'editable': (255, 219, 241)}[field]
    pixels = image.load()
    points = [(x,y) for y in range(image.height) for x in range(image.width) if pixels[x,y] == color]
    assert len(points) > 100, 'Target color not visible in actual screenshot'
    x = (min(p[0] for p in points) + max(p[0] for p in points)) // 2
    y = (min(p[1] for p in points) + 3 * max(p[1] for p in points)) // 4
    assert pixels[x,y] == color, 'Screenshot target is occluded'
    result = {'display': {'id': monitor['name'], 'x': monitor['x'], 'y': monitor['y'],
                         'width': round(monitor['width']/monitor['scale']), 'height': round(monitor['height']/monitor['scale'])},
              'point': {'x':x, 'y':y, 'width':image.width, 'height':image.height},
              'scale':monitor['scale'], 'screenshot':str(path)}
    print(json.dumps(result))


def discover():
    for name in ('/usr/bin/helium', '/usr/bin/chromium', '/usr/bin/chromium-browser'):
        if pathlib.Path(name).is_file():
            return name
    # Inspect executable links only, never reuse a process/profile/debug endpoint.
    for path in pathlib.Path('/proc').glob('[0-9]*/exe'):
        try:
            exe = path.resolve(strict=True)
            if exe.name in ('helium', 'chromium'):
                return str(exe)
        except (OSError, RuntimeError):
            pass
    raise SystemExit('No actual Helium/Chromium executable discovered; use --browser')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--nested', action='store_true', required=True)
    parser.add_argument('--render-node', type=pathlib.Path, required=True)
    parser.add_argument('--browser')
    parser.add_argument('--scale', type=float, choices=(1.0, 1.25), default=1.0)
    parser.add_argument('--private-headless-output', action='store_true',
                        help='Use one headless test output inside nested Hyprland (remove presentation output)')
    args = parser.parse_args()
    assert os.environ.get('NEOISM_LIVE_KEYBOARD_TEST') == '1', 'Explicit opt-in required'
    render = args.render_node.resolve(strict=True)
    info = render.stat()
    assert render.parent == pathlib.Path('/dev/dri') and render.name.startswith('renderD')
    assert stat.S_ISCHR(info.st_mode) and os.major(info.st_rdev) == 226 and 128 <= os.minor(info.st_rdev) <= 255
    host = (pathlib.Path(os.environ['XDG_RUNTIME_DIR']) / os.environ['WAYLAND_DISPLAY']).resolve(strict=True)
    assert stat.S_ISSOCK(host.stat().st_mode)
    browser = args.browser or discover()
    run = pathlib.Path(tempfile.mkdtemp(prefix='nb-live-'))
    for name in ('r', 'home', 'config', 'cache', 'data', 'state'):
        (run / name).mkdir(mode=0o700)
    cargo = pathlib.Path(os.environ.get('CARGO_HOME', pathlib.Path.home() / '.cargo'))
    rustup = pathlib.Path(os.environ.get('RUSTUP_HOME', pathlib.Path.home() / '.rustup'))
    target = ROOT / 'target/computer-keymap-regression'
    target.mkdir(parents=True, exist_ok=True)
    sources = ROOT / 'neoism-agent/crates/neoism-agent-server/src/computer_use'
    (run / 'sources').mkdir()
    for name in ('linux_text.rs', 'shortcuts.rs', 'linux_browser_live_tests.rs', 'linux_pointer.rs', 'linux_clipboard.rs'):
        shutil.copyfile(sources / name, run / 'sources' / name)
    (run / 'sources.sha256').write_text(''.join(f'{hashlib.sha256((run / "sources" / name).read_bytes()).hexdigest()}  {name}\n'
        for name in ('linux_text.rs', 'shortcuts.rs', 'linux_browser_live_tests.rs', 'linux_pointer.rs', 'linux_clipboard.rs')))
    command = ['bwrap', '--die-with-parent', '--bind', '/', '/', '--dev', '/dev', '--tmpfs', '/mnt',
               '--', '/usr/bin/python3', str(pathlib.Path(__file__).resolve()), '--client', str(run), browser]
    child = run / 'client.sh'
    child.write_text('#!/bin/sh\n' + shlex.join(command) + ' >' + shlex.quote(str(run / 'session.log')) + ' 2>&1\n'
                     + 'hyprctl -i "$HYPRLAND_INSTANCE_SIGNATURE" dispatch exit\n')
    config = run / 'hyprland.conf'
    config.write_text(f'''monitor = , 1280x800@60, 0x0, {args.scale}
exec-once = /bin/sh {shlex.quote(str(child))}
xwayland {{
 enabled = false
}}
animations {{
 enabled = false
}}
misc {{
 disable_hyprland_logo = true
 disable_splash_rendering = true
}}
''')
    env = {'PATH': f'{cargo}/bin:/usr/bin:/bin', 'HOME': str(run / 'home'),
           'CARGO_HOME': str(cargo), 'RUSTUP_HOME': str(rustup), 'LANG': 'C.UTF-8',
           'XDG_RUNTIME_DIR': str(run / 'r'), 'WAYLAND_DISPLAY': '/mnt/wayland',
           'XDG_CURRENT_DESKTOP': 'Hyprland', 'XDG_SESSION_TYPE': 'wayland',
           'HYPRLAND_NO_SD_VARS': '1', 'HYPRLAND_NO_SD_NOTIFY': '1', 'HYPRLAND_NO_CRASHREPORTER': '1',
           'LIBSEAT_BACKEND': 'noop', 'AQ_DRM_DEVICES': str(render), 'NEOISM_LIVE_KEYBOARD_TEST': '1',
           'NEOISM_BROWSER_SCALE': str(args.scale),
           'NEOISM_BROWSER_HEADLESS_OUTPUT': '1' if args.private_headless_output else '0'}
    for key, directory in [('CONFIG', 'config'), ('CACHE', 'cache'), ('DATA', 'data'), ('STATE', 'state')]:
        env[f'XDG_{key}_HOME'] = str(run / directory)
    command = ['bwrap', '--die-with-parent', '--unshare-all', '--new-session', '--ro-bind', '/', '/',
               '--dev', '/dev', '--proc', '/proc', '--tmpfs', '/run', '--tmpfs', '/tmp', '--tmpfs', '/mnt',
               '--ro-bind', str(host), '/mnt/wayland', '--dev-bind', str(render), str(render),
               '--bind', str(run), str(run), '--bind', str(cargo), str(cargo), '--bind', str(target), str(target),
               '--chdir', str(ROOT), '--', 'dbus-run-session', '--', 'Hyprland', '--config', str(config)]
    (run / 'launch.txt').write_text(shlex.join(command) + '\n' + repr(env))
    print(f'Isolated browser logs: {run}', flush=True)
    with (run / 'hyprland.log').open('w') as log:
        proc = subprocess.Popen(command, env=env, stdout=log, stderr=subprocess.STDOUT)
        try:
            proc.wait(timeout=210)
        except (subprocess.TimeoutExpired, KeyboardInterrupt):
            proc.kill()  # PID namespace teardown also kills all fresh browser descendants.
            proc.wait()
    after = ''.join(f'{hashlib.sha256((sources / name).read_bytes()).hexdigest()}  {name}\n'
                    for name in ('linux_text.rs', 'shortcuts.rs', 'linux_browser_live_tests.rs', 'linux_pointer.rs', 'linux_clipboard.rs'))
    (run / 'sources-after.sha256').write_text(after)
    if after != (run / 'sources.sha256').read_text():
        print('WARNING: sources changed during run; do not label this as a frozen-source baseline.')
    for name in ('session.log', 'browser.log', 'test.log'):
        path = run / name
        if path.exists():
            print(f'--- {name} ---\n' + '\n'.join(path.read_text(errors='replace').splitlines()[-35:]))
    status = run / 'test.status'
    if not status.exists():
        print('BLOCKED: no test completion; inspect retained logs (not a claimed regression failure).')
        return 1
    return int(status.read_text())


if __name__ == '__main__':
    if len(sys.argv) > 1 and sys.argv[1] == '--client':
        client(pathlib.Path(sys.argv[2]), sys.argv[3])
    elif len(sys.argv) > 1 and sys.argv[1] == '--observe-pointer':
        observe_pointer(pathlib.Path(sys.argv[2]), sys.argv[3])
    else:
        sys.exit(main())
