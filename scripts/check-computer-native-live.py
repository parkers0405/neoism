#!/usr/bin/env python3
"""Bounded real-app acceptance in a private nested Hyprland (Linux only).
No accounts/instances are read. Minecraft runs vanilla, offline, fresh gameDir.
GTK and Minecraft exercise the same production prepare_text/execute_text facade.
Existing browser launcher has no reusable isolation class; this launcher does not
import its main (which hardwires browser startup). No host input tools are used.
"""
import argparse
import hashlib
import json
import os
import pathlib
import shlex
import shutil
import stat
import subprocess
import sys
import tempfile
import time

ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT = pathlib.Path(__file__).resolve()

def build(run):
    src = run / 'src'
    src.mkdir()
    production = ROOT / 'neoism-agent/crates/neoism-agent-server/src/computer_use'
    # Snapshot only keyboard dependencies, retaining relative Rust submodule paths.
    for name in ('latency.rs', 'typing.rs', 'linux_pointer.rs', 'linux_text.rs', 'layout_plan.rs', 'shortcuts.rs', 'linux_clipboard.rs', 'linux_keyboard_tests.rs', 'linux_keyboard_live_tests.rs'):
        shutil.copyfile(production / name, src / name)
    if (production / 'linux_text').exists():
        shutil.copytree(production / 'linux_text', src / 'linux_text')
    shutil.copyfile(SCRIPT.with_name('native-typing-live-tests.rs'), src / 'native_tests.rs')
    (run / 'sources.sha256').write_text(''.join(f'{hashlib.sha256(p.read_bytes()).hexdigest()}  {p.relative_to(src)}\n' for p in sorted(src.rglob('*.rs'))))
    (run / 'Cargo.toml').write_text('''[package]
name="neoism-native-typing-acceptance"
version="0.0.0"
edition="2024"
[dependencies]
anyhow="1"
enigo={version="0.6",default-features=false,features=["wayland"]}
libc="0.2"
log="0.4"
serde_json="1"
serde={version="1",features=["derive"]}
xkbcommon="0.9"
xkeysym="0.2"
tempfile="3"
rand="0.8"
wayland-client="0.31"
wayland-protocols={version="0.32",features=["client","unstable"]}
wayland-protocols-wlr={version="0.3",features=["client"]}
wayland-protocols-misc={version="0.3",features=["client"]}
''')
    main_source = (production.parent / 'computer_use.rs').read_text()
    parsers = main_source[main_source.index('fn modifier_key('):main_source.index('fn coordinates(')]
    parsers = parsers[:parsers.rfind('}') + 1]
    (run / 'key-parsers.rs').write_text(parsers)
    (src / 'lib.rs').write_text('#![allow(dead_code)]\nuse anyhow::bail;\n' + parsers + '''
pub use enigo::{InputError,InputResult,Key};
mod latency;
mod linux_text;
mod shortcuts;
mod linux_clipboard;
mod typing;
#[derive(Clone, Debug, PartialEq)]
struct Display {id:String,x:i32,y:i32,width:u32,height:u32}
mod linux_pointer;
#[cfg(test)] mod native_tests;
''')
    (run / 'sources.sha256').write_text(''.join(f'{hashlib.sha256(p.read_bytes()).hexdigest()}  {p.relative_to(src)}\n' for p in sorted(src.rglob('*.rs'))))
    env = os.environ | {'CARGO_TARGET_DIR': str(ROOT / 'target/computer-keymap-regression')}
    with (run / 'build.log').open('w') as log:
        result = subprocess.run(['cargo', 'test', '--offline', '--no-run', '--message-format=json', '--manifest-path', str(run / 'Cargo.toml')], env=env, stdout=subprocess.PIPE, stderr=log, timeout=150)
    (run / 'build.jsonl').write_bytes(result.stdout)
    if result.returncode:
        raise RuntimeError(f'Native test compile failed: {run}/build.log')
    artifacts = [json.loads(line) for line in result.stdout.splitlines()]
    exe = next(a['executable'] for a in artifacts if a.get('executable') and a.get('profile', {}).get('test'))
    shutil.copyfile(exe, run / 'native-test')
    (run / 'native-test').chmod(0o700)

def minecraft(run, prism):
    meta = json.loads((prism / 'meta/net.minecraft/26.2.json').read_text())
    jars = []
    lwjgl = json.loads((prism / 'meta/org.lwjgl3/3.4.1.json').read_text())
    for lib in meta['libraries'] + lwjgl['libraries']:
        allowed = not lib.get('rules')
        for rule in lib.get('rules', []):
            if rule.get('os', {}).get('name', 'linux') == 'linux':
                allowed = rule['action'] == 'allow'
        if not allowed:
            continue
        artifact = lib.get('downloads', {}).get('artifact')
        if not artifact:
            continue
        # Prism's Maven name can differ from the artifact URL (native jars).
        parts = lib['name'].split(':')
        group, artifact_name, version = parts[:3]
        classifier = '-' + parts[3] if len(parts) > 3 else ''
        relative = f"{group.replace('.', '/')}/{artifact_name}/{version}/{artifact_name}-{version}{classifier}.jar"
        path = prism / 'libraries' / relative
        if not path.is_file():
            raise RuntimeError(f'Missing installed library (no downloads): {path}')
        jars.append('/opt/mc/libraries/' + relative)
    jars.append('/opt/mc/libraries/com/mojang/minecraft/26.2/minecraft-26.2-client.jar')
    shutil.copyfile(SCRIPT.with_name('NativeMinecraftObserver.java'), run / 'NativeMinecraftObserver.java')
    jdk = prism / 'java/java-runtime-epsilon/bin'
    subprocess.run([str(jdk / 'javac'), '-d', str(run / 'observer-classes'), str(run / 'NativeMinecraftObserver.java')], check=True, timeout=20)
    (run / 'observer.mf').write_text('Premain-Class: NativeMinecraftObserver\n\n')
    subprocess.run([str(jdk / 'jar'), 'cfm', str(run / 'observer.jar'), str(run / 'observer.mf'), '-C', str(run / 'observer-classes'), '.'], check=True, timeout=15)
    command = ['/opt/mc/java/java-runtime-epsilon/bin/java', '-Xmx1536m', '-Djava.net.preferIPv4Stack=true',
               '-javaagent:' + str(run / 'observer.jar') + '=' + str(run / 'report.json'),
               '-cp', ':'.join(jars), 'net.minecraft.client.main.Main', '--username', 'OfflineAcceptance',
               '--version', '26.2', '--gameDir', str(run / 'game'), '--assetsDir', '/opt/mc/assets',
               '--assetIndex', meta['assetIndex']['id'], '--uuid', '00000000000000000000000000000001',
               '--accessToken', '0', '--userType', 'legacy', '--versionType', 'release', '--width', '1000', '--height', '650']
    (run / 'minecraft-command.json').write_text(json.dumps(command))

def client(run, app):
    runtime = pathlib.Path(os.environ['XDG_RUNTIME_DIR'])
    assert runtime == run / 'r'
    display = pathlib.Path(os.environ['WAYLAND_DISPLAY'])
    assert len(display.parts) == 1 and not display.is_absolute()
    assert stat.S_ISSOCK((runtime / display).stat().st_mode)
    assert not pathlib.Path('/mnt/wayland').exists()
    assert not pathlib.Path('/dev/input').exists()
    assert not list(pathlib.Path('/dev/dri').glob('*'))
    assert not pathlib.Path('/home/parkersettle/.local/share/PrismLauncher').exists()
    signature = os.environ['HYPRLAND_INSTANCE_SIGNATURE']
    assert signature and '/' not in signature
    assert stat.S_ISSOCK((runtime / 'hypr' / signature / '.socket.sock').stat().st_mode)
    monitors = json.loads(subprocess.check_output(['hyprctl', '-i', signature, '-j', 'monitors']))
    assert len(monitors) == 1
    # Remove the nested presentation output: host keyboard focus/events must not
    # contaminate the acceptance app. Capture a private headless output instead.
    # Let the outer backend finish its initial configure/reconfigure before
    # destroying its presentation output. No app or injector exists yet.
    time.sleep(1)
    subprocess.run(['hyprctl', '-i', signature, 'output', 'create', 'headless', 'NATIVE-TEST'], check=True)
    subprocess.run(['hyprctl', '-i', signature, 'keyword', 'monitor', 'NATIVE-TEST,1280x800@60,0x0,1'], check=True)
    subprocess.run(['hyprctl', '-i', signature, 'output', 'remove', monitors[0]['name']], check=True)
    print('VERIFIED private endpoints, no host home/socket/input/DRM; network namespace isolated; headless output', flush=True)
    env = os.environ | {'GDK_BACKEND': 'wayland', 'LIBGL_ALWAYS_SOFTWARE': '1',
                        'GSK_RENDERER': 'cairo', 'GTK_A11Y': 'none', 'GTK_USE_PORTAL': '0', 'GIO_USE_VFS': 'local'}
    command = ([sys.executable, str(run / 'native-typing-gtk.py'), str(run)] if app == 'gtk'
               else json.loads((run / 'minecraft-command.json').read_text()))
    with (run / 'app.log').open('w') as log:
        proc = subprocess.Popen(command, env=env, stdout=log, stderr=subprocess.STDOUT)
        try:
            deadline = time.monotonic() + (60 if app == 'minecraft' else 25)
            while True:
                report = json.loads((run / 'report.json').read_text()) if (run / 'report.json').exists() else {}
                ready = report.get('focused') and (app == 'gtk' or (report.get('widgets') and not report.get('overlay')))
                if ready:
                    break
                assert proc.poll() is None and time.monotonic() < deadline, f'{app} observer readiness failed; inspect app/observer logs'
                time.sleep(.1)
            env |= {'NEOISM_NATIVE_LIVE': '1', 'NEOISM_NATIVE_RUNTIME': str(runtime),
                    'NEOISM_NATIVE_REPORT': str(run / 'report.json'), 'NEOISM_NATIVE_PID': str(proc.pid),
                    'NEOISM_NATIVE_RUN': str(run), 'NEOISM_NATIVE_APP': app}
            test = 'native_minecraft_editbox_roundtrip' if app == 'minecraft' else 'native_gtk_existing_layout_roundtrip'
            with (run / 'test.log').open('w') as output:
                result = subprocess.run([str(run / 'native-test'), 'native_tests::' + test, '--exact', '--ignored', '--nocapture'], env=env, stdout=output, stderr=subprocess.STDOUT, timeout=150)
            (run / 'test.status').write_text(str(result.returncode))
            # Terminating the private process is the end of this test. No Return
            # key or final Create New World button is ever dispatched.
        finally:
            if proc.poll() is None:
                proc.terminate()
                try:
                    proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    proc.kill()
                    proc.wait()

def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--nested', action='store_true', required=True)
    p.add_argument('--render-node', type=pathlib.Path, required=True)
    p.add_argument('--app', choices=['gtk', 'minecraft'], default='gtk')
    p.add_argument('--allow-private-clipboard', action='store_true')
    args = p.parse_args()
    assert os.environ.get('NEOISM_LIVE_KEYBOARD_TEST') == '1', 'Explicit opt-in required'
    render = args.render_node.resolve(strict=True)
    st = render.stat()
    assert render.parent == pathlib.Path('/dev/dri') and render.name.startswith('renderD')
    assert stat.S_ISCHR(st.st_mode) and os.major(st.st_rdev) == 226 and 128 <= os.minor(st.st_rdev) <= 255
    host = (pathlib.Path(os.environ['XDG_RUNTIME_DIR']) / os.environ['WAYLAND_DISPLAY']).resolve(strict=True)
    assert stat.S_ISSOCK(host.stat().st_mode)
    run = pathlib.Path(tempfile.mkdtemp(prefix='nn-live-'))
    print(f'Native acceptance artifacts: {run}', flush=True)
    for name in ('r', 'home', 'config', 'cache', 'data', 'state', 'game'):
        (run / name).mkdir(mode=0o700)
    shutil.copyfile(SCRIPT, run / SCRIPT.name)
    shutil.copyfile(SCRIPT.with_name('native-typing-gtk.py'), run / 'native-typing-gtk.py')
    prism = pathlib.Path.home() / '.local/share/PrismLauncher'
    build(run)
    if args.app == 'minecraft':
        minecraft(run, prism)
    inner = ['bwrap', '--die-with-parent', '--bind', '/', '/', '--dev', '/dev', '--tmpfs', '/mnt', '--',
             '/usr/bin/python3', str(run / SCRIPT.name), '--client', str(run), args.app]
    (run / 'client.sh').write_text('#!/bin/sh\n' + shlex.join(inner) + f' >{run}/session.log 2>&1\n' + 'hyprctl -i "$HYPRLAND_INSTANCE_SIGNATURE" dispatch exit\n')
    (run / 'hyprland.conf').write_text(f'''monitor = ,1280x800@60,0x0,1
exec-once = /bin/sh {run}/client.sh
xwayland {{
 enabled = {'true' if args.app == 'minecraft' else 'false'}
}}
animations {{
 enabled = false
}}
misc {{
 disable_hyprland_logo = true
 disable_splash_rendering = true
}}
''')
    env = {'PATH': '/usr/bin:/bin', 'HOME': str(run / 'home'), 'LANG': 'C.UTF-8',
           'XDG_RUNTIME_DIR': str(run / 'r'), 'WAYLAND_DISPLAY': '/mnt/wayland',
           'XDG_CURRENT_DESKTOP': 'Hyprland', 'XDG_SESSION_TYPE': 'wayland',
           'HYPRLAND_NO_SD_VARS': '1', 'HYPRLAND_NO_SD_NOTIFY': '1', 'HYPRLAND_NO_CRASHREPORTER': '1',
           'LIBSEAT_BACKEND': 'noop', 'AQ_DRM_DEVICES': str(render),
           'NEOISM_NATIVE_CLIPBOARD_POLICY': 'allow' if args.allow_private_clipboard else 'deny'}
    for key, directory in [('CONFIG', 'config'), ('CACHE', 'cache'), ('DATA', 'data'), ('STATE', 'state')]:
        env[f'XDG_{key}_HOME'] = str(run / directory)
    command = ['bwrap', '--die-with-parent', '--unshare-all', '--new-session', '--ro-bind', '/', '/',
               '--dev', '/dev', '--proc', '/proc', '--tmpfs', '/run', '--tmpfs', '/tmp', '--tmpfs', '/mnt',
               '--tmpfs', '/home', '--tmpfs', '/root', '--tmpfs', '/opt',
               '--ro-bind', str(host), '/mnt/wayland', '--dev-bind', str(render), str(render), '--bind', str(run), str(run)]
    if args.app == 'minecraft':
        for name in ('libraries', 'assets', 'java'):
            command += ['--ro-bind', str(prism / name), '/opt/mc/' + name]
    command += ['--chdir', str(run), '--', 'dbus-run-session', '--', 'Hyprland', '--config', str(run / 'hyprland.conf')]
    (run / 'launch.json').write_text(json.dumps({'command': command, 'environment': env}, indent=2))
    with (run / 'hyprland.log').open('w') as log:
        proc = subprocess.Popen(command, env=env, stdout=log, stderr=subprocess.STDOUT)
        try:
            proc.wait(timeout=230)
        except (subprocess.TimeoutExpired, KeyboardInterrupt):
            proc.kill()
            proc.wait()
    (run / 'compositor.status').write_text(str(proc.returncode))
    status = run / 'test.status'
    production = ROOT / 'neoism-agent/crates/neoism-agent-server/src/computer_use'
    changed = [p.name for p in (run / 'src').glob('*.rs') if (production / p.name).is_file() and p.read_bytes() != (production / p.name).read_bytes()]
    main_source = (production.parent / 'computer_use.rs').read_text()
    parsers = main_source[main_source.index('fn modifier_key('):main_source.index('fn coordinates(')]
    parsers = parsers[:parsers.rfind('}') + 1]
    if parsers != (run / 'key-parsers.rs').read_text():
        changed.append('computer_use.rs:key-parsers')
    after = ''.join(f'{hashlib.sha256((production / p.name).read_bytes()).hexdigest()}  {p.name}\n' for p in sorted((run / 'src').glob('*.rs')) if (production / p.name).is_file())
    (run / 'production-after.sha256').write_text(after)
    worlds = list((run / 'game').rglob('level.dat'))
    (run / 'source-check.json').write_text(json.dumps({'changed_during_run': changed, 'created_world_level_dat': [str(p) for p in worlds]}))
    if worlds:
        raise RuntimeError('Unexpected world data in PRIVATE test game directory')
    if changed:
        print('WARNING production source changed during test:', changed)
    print(status.read_text() if status.exists() else f'No result; inspect {run}/session.log')
    return 0 if status.exists() and status.read_text() == '0' and not changed else 1

if __name__ == '__main__':
    if len(sys.argv) > 1 and sys.argv[1] == '--client':
        client(pathlib.Path(sys.argv[2]), sys.argv[3])
    else:
        sys.exit(main())
