#!/usr/bin/env python3
"""Package and smoke-test the native GPUIX bundle on the current platform."""
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tarfile
import tempfile
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parent.parent
DIST = ROOT / 'neoism-frontend/gpuix/dist'
VERSION = tomllib.loads((ROOT / 'Cargo.toml').read_text())['workspace']['package']['version']
SYSTEM = {'Linux': 'linux', 'Darwin': 'darwin', 'Windows': 'windows'}[platform.system()]
ARCH = 'aarch64' if platform.machine().lower() in ('arm64', 'aarch64') else 'x86_64'
SUFFIX = '.exe' if SYSTEM == 'windows' else ''
LIBRARIES = {'linux': ['libneoism_gpuix_core.so', 'libghostty-vt.so'],
             'darwin': ['libneoism_gpuix_core.dylib', 'libghostty-vt.dylib'],
             'windows': ['neoism_gpuix_core.dll', 'ghostty-vt.dll']}[SYSTEM]
OUT = ROOT / 'release'
OUT.mkdir(exist_ok=True)
NAME = f'neoism-{SYSTEM}-{ARCH}'

def run(*args):
    return subprocess.check_output([str(arg) for arg in args], text=True, stderr=subprocess.STDOUT).strip()

with tempfile.TemporaryDirectory(prefix='neoism-package-') as temporary:
    stage = Path(temporary)
    bundle = stage / NAME
    bundle.mkdir()
    for name in ['neoism' + SUFFIX, 'neoism-workspace-daemon' + SUFFIX,
                 'neoism-agent' + SUFFIX, 'neoism-agent-lua-runner' + SUFFIX, *LIBRARIES, 'LICENSE.ghostty', 'LICENSE', 'NOTICE']:
        shutil.copy2(DIST / name, bundle / name)
    for name in ['licenses', 'web']:
        shutil.copytree(DIST / name, bundle / name)
    assert (bundle / 'web/agent-gui/index.html').is_file()
    application = bundle / 'web/app'
    manifest = json.loads((application / 'manifest.json').read_text())
    assert manifest['entry'] in manifest['files']
    assert manifest['entry'] + '.jsc' in manifest['files']
    for name, digest in manifest['files'].items():
        file = application / name
        assert file.resolve().is_relative_to(application.resolve())
        assert file.is_file(), f'Application resource is missing: {name}'
        assert hashlib.file_digest(file.open('rb'), 'sha256').hexdigest() == digest
    for stale in application.rglob('*'):
        if stale.is_file() and stale.relative_to(application).as_posix() not in {*manifest['files'], 'manifest.json'}:
            stale.unlink()
    runtime = bundle / 'web/native'
    runtime.mkdir(exist_ok=True)
    native_platform = 'win32' if SYSTEM == 'windows' else SYSTEM
    native_arch = 'arm64' if ARCH == 'aarch64' else 'x64'
    display_stamp = json.loads((ROOT / 'neoism-frontend/gpuix/artifacts' / f'gpuix-display-{native_platform}-{native_arch}.json').read_text())
    display_library = runtime / display_stamp['artifact']
    assert display_library.is_file(), 'Native display library is missing from the bundle'
    assert hashlib.file_digest(display_library.open('rb'), 'sha256').hexdigest() == display_stamp['sha256']
    for stale in runtime.glob('gpuix-display-*.node'):
        if stale.name != display_library.name:
            stale.unlink()
    for name in LIBRARIES:
        shutil.copy2(bundle / name, runtime / name)
    shutil.copytree(bundle / 'licenses', runtime / 'licenses')
    for name in ['LICENSE', 'NOTICE', 'LICENSE.ghostty']:
        shutil.copy2(bundle / name, runtime / name)
    # Immutable old updaters require web/index.html and copy the whole tree.
    (bundle / 'web/index.html').write_text('<!doctype html><meta http-equiv="refresh" content="0;url=agent-gui/">')
    run(bundle / ('neoism' + SUFFIX), '--neoism-verify-runtime')
    assert VERSION in run(bundle / ('neoism' + SUFFIX), '--version')
    if SYSTEM == 'linux':
        subprocess.run(['cargo', 'test', '--locked', '--release', '-p', 'neoism-update-helper', '--bin', 'neoism-update-helper', 'legacy_installer_keeps_gpuix_runtime'], cwd=ROOT, env={**os.environ, 'NEOISM_TEST_UPDATE_PAYLOAD': str(bundle)}, check=True)
    commit = run('git', 'rev-parse', 'HEAD')
    metadata = json.dumps({'version': VERSION, 'commit': commit,
        'platform': SYSTEM, 'architecture': ARCH}, indent=2) + '\n'
    (bundle / 'build-info.json').write_text(metadata)
    (OUT / f'{NAME}.build-info.json').write_text(metadata)
    if SYSTEM == 'linux':
        shutil.copy2(ROOT / 'scripts/install-gpuix-bundle.sh', bundle / 'install-bundle.sh')
        shutil.copy2(ROOT / 'misc/neoism.desktop', bundle / 'neoism.desktop')
        shutil.copy2(ROOT / 'neoism-frontend/shared/assets/icons/neoism.png', bundle / 'neoism.png')
    elif SYSTEM == 'darwin':
        app = bundle / 'Neoism.app'
        contents = app / 'Contents'
        contents.mkdir(parents=True)
        macos = contents / 'MacOS'
        macos.mkdir()
        for name in ['neoism', 'neoism-workspace-daemon', 'neoism-agent', 'neoism-agent-lua-runner']:
            shutil.copy2(bundle / name, macos / name)
        resources = contents / 'Resources'
        resources.mkdir()
        shutil.copytree(bundle / 'web', resources / 'web')
        shutil.copy2(ROOT / 'misc/macos/neoism.icns', resources / 'neoism.icns')
        plist = (ROOT / 'misc/macos/Info.plist').read_text()
        (contents / 'Info.plist').write_text(plist.replace('{{RELEASE_VERSION}}', VERSION).replace('{{VERSION}}', VERSION.split('-')[0]))
        for name in [*LIBRARIES, display_library.name, 'neoism-update-helper']:
            run('codesign', '--force', '--sign', '-', resources / 'web/native' / name)
        for name in ['neoism', 'neoism-workspace-daemon', 'neoism-agent', 'neoism-agent-lua-runner']:
            run('codesign', '--force', '--sign', '-', macos / name)
        run('codesign', '--force', '--deep', '--sign', '-', app)
        run('codesign', '--verify', '--deep', '--strict', app)
        assert VERSION in run(macos / 'neoism', '--version')
        run(macos / 'neoism', '--neoism-verify-runtime')
        dmg_source = stage / 'dmg'
        dmg_source.mkdir()
        shutil.copytree(app, dmg_source / 'Neoism.app')
        os.symlink('/Applications', dmg_source / 'Applications')
        run('hdiutil', 'create', '-volname', 'Neoism', '-srcfolder', dmg_source,
            '-ov', '-format', 'UDZO', OUT / f'{NAME}.dmg')
    archive = OUT / (NAME + ('.zip' if SYSTEM == 'windows' else '.tar.gz'))
    if SYSTEM == 'windows':
        shutil.copytree(bundle / 'web', DIST / 'web', dirs_exist_ok=True)
        (bundle / 'README.txt').write_text('Extract the entire folder, then run neoism.exe. Keep the DLLs, services, web and licenses beside it.\n')
        with zipfile.ZipFile(archive, 'w', zipfile.ZIP_DEFLATED) as output:
            for path in bundle.rglob('*'):
                if path.is_file():
                    output.write(path, path.relative_to(stage))
    else:
        with tarfile.open(archive, 'w:gz') as output:
            payload = bundle
            output.add(payload, arcname=payload.name)
    # Validate the archive, not just the staging directory.
    extracted = stage / 'extracted'
    extracted.mkdir()
    if SYSTEM == 'windows':
        with zipfile.ZipFile(archive) as source:
            source.extractall(extracted)
    else:
        with tarfile.open(archive) as source:
            source.extractall(extracted, filter='data')
    installed = extracted / NAME / 'Neoism.app/Contents/MacOS' if SYSTEM == 'darwin' else extracted / NAME
    assert VERSION in run(installed / ('neoism' + SUFFIX), '--version')
    run(installed / ('neoism' + SUFFIX), '--help')
    run(installed / ('neoism' + SUFFIX), '--neoism-verify-runtime')
    run(installed / ('neoism' + SUFFIX), 'update', '--help')
    run(installed / ('neoism' + SUFFIX), '--update', 'nightly', '--help')
    for artifact in OUT.glob(NAME + '.*'):
        if artifact.suffix == '.sha256':
            continue
        digest = hashlib.file_digest(artifact.open('rb'), 'sha256').hexdigest()
        Path(str(artifact) + '.sha256').write_text(f'{digest}  {artifact.name}\n')
        print(artifact)
