#!/usr/bin/env python3
"""Build, stage, test and package the Neoism release bundle on the current platform.

The app is the Rune frontend (`rune/app`), shipped under the name every installer, updater
and desktop integration already knows: `neoism`. Beside it go the services it starts (the
workspace daemon, the agent CLI and its Lua runner), the terminal engine (libghostty-vt),
the update helper, the docs site and the notices.

The staged tree keeps the layout deployed updaters validate before they will install a
bundle: the four executables by name, `web/index.html`, `web/agent-gui/index.html`,
`web/app/manifest.json`, `web/native/` (libraries and the update helper) and
`resources/docs`. `NEOISM_RELEASE_DIST` moves the staging directory (the Windows MSI
scripts read the default one).
"""
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent
DIST = Path(os.environ.get('NEOISM_RELEASE_DIST') or ROOT / 'neoism-frontend/gpuix/dist').resolve()
ARTIFACTS = ROOT / 'neoism-frontend/gpuix/artifacts'
RUNE = ROOT / 'rune/app'
# Where the app's own workspace builds; a developer's machine may already have one elsewhere.
RUNE_TARGET = Path(os.environ.get('NEOISM_RUNE_TARGET') or RUNE / 'target').resolve()
# The freshly built services; a dry run may point at an installed set instead.
SERVICE_DIR = Path(os.environ.get('NEOISM_SERVICES_DIR') or ROOT / 'target/release').resolve()
SYSTEM = {'Linux': 'linux', 'Darwin': 'macos', 'Windows': 'windows'}[platform.system()]
ARCH = 'aarch64' if platform.machine().lower() in ('arm64', 'aarch64') else 'x86_64'
SUFFIX = '.exe' if SYSTEM == 'windows' else ''
GHOSTTY = {'linux': 'libghostty-vt.so', 'macos': 'libghostty-vt.dylib', 'windows': 'ghostty-vt.dll'}[SYSTEM]
ENV = {**os.environ, 'CARGO_PROFILE_RELEASE_DEBUG': '0', 'CARGO_PROFILE_RELEASE_LTO': 'thin', 'CARGO_PROFILE_RELEASE_CODEGEN_UNITS': '16'}
SERVICES = ['neoism-workspace-daemon', 'neoism-agent', 'neoism-agent-lua-runner']
LICENSES = [
    ('neoism-paint/src/font/resources/CascadiaCode/LICENSE', 'CascadiaCode.txt'),
    ('neoism-paint/src/font/resources/SymbolsNerdFontMono/LICENSE', 'SymbolsNerdFontMono.txt'),
    ('neoism-paint/src/font/resources/Geist/OFL.txt', 'Geist-OFL.txt'),
    ('neoism-paint/src/font/resources/PressStart2P/OFL.txt', 'PressStart2P-OFL.txt'),
    ('vendor/blaze-compat/icu_collator_data-2.4.0/LICENSE', 'ICU4X.txt'),
    ('vendor/blaze-compat/licenses/Tantivy.txt', 'Tantivy.txt'),
    ('vendor/blaze-compat/licenses/Tantivy-AUTHORS.txt', 'Tantivy-AUTHORS.txt'),
    ('vendor/blaze-compat/licenses/Turso.txt', 'Turso.txt'),
]
MARKER = b'<!-- Neoism native runtime resources; no browser UI. -->\n'


def run(*command, cwd=ROOT, env=None):
    command = [str(part) for part in command]
    label = ' '.join(command)
    print(f'::group::{label}' if os.environ.get('GITHUB_ACTIONS') else f'$ {label}', flush=True)
    try:
        subprocess.run(command, cwd=cwd, env=env or ENV, check=True)
    finally:
        if os.environ.get('GITHUB_ACTIONS'):
            print('::endgroup::', flush=True)


def sha256(path):
    return hashlib.file_digest(path.open('rb'), 'sha256').hexdigest()


def download_pinned(url, destination, digest):
    """Only complete bytes matching the reviewed pin are kept, whichever mirror serves them.
    The mirrors are the ones ziglang.org lists; trust comes from the pinned SHA-256."""
    if destination.exists() and sha256(destination) == digest:
        return
    name = url.rsplit('/', 1)[-1]
    pending = destination.with_name(destination.name + '.part')
    failures = []
    for source in (f'https://pkg.hexops.org/zig/{name}?source=neoism', f'https://ziglang.freetls.fastly.net/{name}?source=neoism', url):
        host = source.split('/')[2]
        print(f'Downloading {name} from {host}…', flush=True)
        # curl bounds both the connection and a stalled body.
        result = subprocess.run(['curl.exe' if SYSTEM == 'windows' else 'curl', '--fail', '--location', '--silent', '--show-error',
            '--connect-timeout', '15', '--max-time', '180', '--speed-limit', '1024', '--speed-time', '30', '--output', str(pending), source])
        if result.returncode == 0 and sha256(pending) == digest:
            pending.replace(destination)
            return
        failures.append(f'{host}: ' + ('checksum mismatch' if result.returncode == 0 else f'curl exited {result.returncode}'))
        pending.unlink(missing_ok=True)
    raise SystemExit('Could not download the pinned Zig toolchain: ' + '; '.join(failures))


def build_ghostty():
    """The terminal engine at its pinned revision, with its pinned Zig (`build-ghostty.ts`)."""
    pin_text = (ROOT / 'neoism-frontend/gpuix/native/ghostty-pin.json').read_text()
    pin = json.loads(pin_text)
    target = f'{ARCH}-{SYSTEM}'
    release = pin['zig'].get(target) or sys.exit(f'Pinned Ghostty toolchain does not support {target}')
    ARTIFACTS.mkdir(parents=True, exist_ok=True)
    output, stamp, source = ARTIFACTS / GHOSTTY, ARTIFACTS / f'ghostty-{target}.json', ARTIFACTS / 'ghostty'
    canonical = json.dumps(pin, separators=(',', ':'))
    if output.exists() and (source / 'LICENSE').exists() and stamp.exists() and stamp.read_text() == canonical:
        print(f'Ghostty VT {pin["revision"][:12]} is built.')
        return output
    if not (source / '.git').exists():
        run('git', 'clone', '--depth', '1', pin['repository'], source)
    head = subprocess.check_output(['git', '-C', str(source), 'rev-parse', 'HEAD'], text=True).strip()
    if head != pin['revision']:
        run('git', 'fetch', '--depth', '1', 'origin', pin['revision'], cwd=source)
        run('git', 'checkout', '--detach', pin['revision'], cwd=source)
    tools = ARTIFACTS / f'zig-{pin["zigVersion"]}-{target}'
    zig = tools / ('zig.exe' if SYSTEM == 'windows' else 'zig')
    if not zig.exists():
        archive = ARTIFACTS / f'zig-{pin["zigVersion"]}-{target}{".zip" if SYSTEM == "windows" else ".tar.xz"}'
        download_pinned(release['url'], archive, release['sha256'])
        tools.mkdir(parents=True, exist_ok=True)
        # Windows ships bsdtar, which also extracts the verified ZIP archive.
        run('tar', '-xf', archive, '--strip-components=1', '-C', tools)
    run(zig, 'build', '-Demit-lib-vt', '-Doptimize=ReleaseFast', cwd=source)
    built = source / 'zig-out' / ('bin' if SYSTEM == 'windows' else 'lib') / GHOSTTY
    shutil.copy2(built, output)
    stamp.write_text(canonical)
    return output


def stage(ghostty):
    """Lay the bundle out as installers and deployed updaters expect to find it."""
    if DIST.exists():
        shutil.rmtree(DIST)
    native = DIST / 'web/native'
    native.mkdir(parents=True)
    shutil.copy2(RUNE_TARGET / 'release' / f'neoism-rune{SUFFIX}', DIST / f'neoism{SUFFIX}')
    for name in SERVICES:
        shutil.copy2(SERVICE_DIR / f'{name}{SUFFIX}', DIST / f'{name}{SUFFIX}')
    helper = f'neoism-update-helper{SUFFIX}'
    shutil.copy2(next(path for path in (SERVICE_DIR / helper, SERVICE_DIR / 'web/native' / helper) if path.exists()), native / helper)
    shutil.copy2(ghostty, DIST / GHOSTTY)
    shutil.copy2(ARTIFACTS / 'ghostty/LICENSE', DIST / 'LICENSE.ghostty')
    for name in ('LICENSE', 'NOTICE'):
        shutil.copy2(ROOT / name, DIST / name)
    (DIST / 'licenses').mkdir()
    for source, name in LICENSES:
        shutil.copy2(ROOT / source, DIST / 'licenses' / name)
    # Deployed updaters refuse a bundle without these. Nothing reads their contents: the app
    # is one native executable, and there is no browser UI.
    (DIST / 'web/app').mkdir()
    (DIST / 'web/app/manifest.json').write_text(json.dumps({'runtime': 'rune', 'entry': None, 'files': {}}) + '\n')
    (DIST / 'web/agent-gui').mkdir()
    for marker in ('web/index.html', 'web/agent-gui/index.html'):
        (DIST / marker).write_bytes(MARKER)
    run('node', ROOT / 'scripts/build-docs.mjs', '--out', DIST / 'resources/docs')


def main():
    # `--reuse-services` packages the services already in target/release (a local dry run of
    # the layout); a release always builds and tests them here.
    reuse_services = '--reuse-services' in sys.argv[1:]
    ghostty = build_ghostty()
    if not reuse_services:
        # Shell argument forwarding and process ownership on the actual host, before the long builds.
        run('cargo', 'test', '--locked', '--release', '-p', 'teletypewriter', 'macos_exec_tests', '--lib')
        run('cargo', 'test', '--locked', '--release', '-p', 'neoism-terminal-pty', '--test', 'process_lifecycle')
        run('cargo', 'build', '--locked', '--release', '-p', 'neoism-workspace-daemon', '-p', 'neoism-agent', '-p', 'neoism-update-helper')
        run('cargo', 'test', '--locked', '--release', '-p', 'neoism-update-helper', '--bin', 'neoism-update-helper')
    # The app has its own workspace and toolchain (`rune/app/rust-toolchain.toml`).
    commit = subprocess.check_output(['git', 'rev-parse', '--short', 'HEAD'], cwd=ROOT, text=True).strip()
    # A dry run keeps the developer's own release profile, so it reuses their build.
    rune_env = {**(os.environ if reuse_services else ENV), 'GIT_HASH': commit, 'CARGO_TARGET_DIR': str(RUNE_TARGET)}
    rune_env.pop('RUSTUP_TOOLCHAIN', None)
    run('cargo', 'build', '--locked', '--release', cwd=RUNE, env=rune_env)
    if SYSTEM == 'linux' and not reuse_services:
        run('cargo', 'test', '--locked', '--release', cwd=RUNE, env=rune_env)
    stage(ghostty)
    run(DIST / f'neoism{SUFFIX}', '--version')
    run(sys.executable, ROOT / 'scripts/package-rune-release.py', *(['--reuse-services'] if reuse_services else []))


if __name__ == '__main__':
    main()
