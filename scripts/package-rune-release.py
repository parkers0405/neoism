#!/usr/bin/env python3
"""Package and smoke-test the staged Neoism bundle (`build-rune-release.py`) on this platform:
the archive, the macOS app and disk image, the checksums, and every command the installers
and deployed updaters will run against it."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parent.parent
DIST = Path(os.environ.get('NEOISM_RELEASE_DIST') or ROOT / 'neoism-frontend/gpuix/dist').resolve()
VERSION = tomllib.loads((ROOT / 'Cargo.toml').read_text())['workspace']['package']['version']
SYSTEM = {'Linux': 'linux', 'Darwin': 'darwin', 'Windows': 'windows'}[platform.system()]
ARCH = 'aarch64' if platform.machine().lower() in ('arm64', 'aarch64') else 'x86_64'
SUFFIX = '.exe' if SYSTEM == 'windows' else ''
LIBRARIES = {'linux': ['libghostty-vt.so'], 'darwin': ['libghostty-vt.dylib'], 'windows': ['ghostty-vt.dll']}[SYSTEM]
OUT = Path(os.environ.get('NEOISM_RELEASE_OUT') or ROOT / 'release').resolve()
OUT.mkdir(exist_ok=True)
NAME = f'neoism-{SYSTEM}-{ARCH}'

def verify_windows_payload(bundle):
    spec = importlib.util.spec_from_file_location('windows_bundle', ROOT / 'scripts/windows-bundle.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.verify_windows_bundle(bundle, ROOT / 'target/release')

def run(*args):
    return subprocess.check_output([str(arg) for arg in args], text=True, stderr=subprocess.STDOUT).strip()

with tempfile.TemporaryDirectory(prefix='neoism-package-') as temporary:
    stage = Path(temporary)
    bundle = stage / NAME
    bundle.mkdir()
    for name in ['neoism' + SUFFIX, 'neoism-workspace-daemon' + SUFFIX,
                 'neoism-agent' + SUFFIX, 'neoism-agent-lua-runner' + SUFFIX, *LIBRARIES, 'LICENSE.ghostty', 'LICENSE', 'NOTICE']:
        shutil.copy2(DIST / name, bundle / name)
    run('node', ROOT / 'scripts/build-docs.mjs', '--verify', DIST / 'resources/docs')
    for name in ['licenses', 'web/app', 'web/native', 'resources/docs']:
        shutil.copytree(DIST / name, bundle / name)
    # The app is one native executable: the manifest only tells deployed updaters, which
    # require it, that there is nothing else to verify.
    manifest = json.loads((bundle / 'web/app/manifest.json').read_text())
    assert manifest['runtime'] == 'rune' and manifest['files'] == {}
    assert [path.name for path in (bundle / 'web/app').iterdir()] == ['manifest.json']
    runtime = bundle / 'web/native'
    assert (runtime / ('neoism-update-helper' + SUFFIX)).is_file(), 'The update helper is missing from the bundle'
    for name in LIBRARIES:
        shutil.copy2(bundle / name, runtime / name)
    shutil.copytree(bundle / 'licenses', runtime / 'licenses')
    for name in ['LICENSE', 'NOTICE', 'LICENSE.ghostty']:
        shutil.copy2(bundle / name, runtime / name)
    # Immutable old Linux updaters require BOTH HTML paths and copy the whole tree.
    # Only native trees were copied above: never preserve an old browser GUI.
    (bundle / 'web/agent-gui').mkdir()
    for marker in ['web/index.html', 'web/agent-gui/index.html']:
        (bundle / marker).write_bytes(b'<!-- Neoism native runtime resources; no browser UI. -->\n')
    service_hashes = verify_windows_payload(bundle) if SYSTEM == 'windows' else {}
    run(bundle / ('neoism' + SUFFIX), '--neoism-verify-runtime')
    assert VERSION in run(bundle / ('neoism' + SUFFIX), '--version')
    if SYSTEM == 'linux' and '--reuse-services' not in sys.argv[1:]:
        subprocess.run(['cargo', 'test', '--locked', '--release', '-p', 'neoism-update-helper', '--bin', 'neoism-update-helper', 'legacy_installer_keeps_native_runtime'], cwd=ROOT, env={**os.environ, 'NEOISM_TEST_UPDATE_PAYLOAD': str(bundle)}, check=True)
    commit = run('git', 'rev-parse', 'HEAD')
    metadata = json.dumps({'version': VERSION, 'commit': commit,
        'platform': SYSTEM, 'architecture': ARCH, 'services': service_hashes}, indent=2) + '\n'
    (bundle / 'build-info.json').write_text(metadata)
    (OUT / f'{NAME}.build-info.json').write_text(metadata)
    if SYSTEM == 'linux':
        shutil.copy2(ROOT / 'scripts/install-neoism-bundle.sh', bundle / 'install-bundle.sh')
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
        shutil.copytree(bundle / 'resources/docs', resources / 'resources/docs')
        shutil.copy2(ROOT / 'misc/macos/neoism.icns', resources / 'neoism.icns')
        plist = (ROOT / 'misc/macos/Info.plist').read_text()
        (contents / 'Info.plist').write_text(plist.replace('{{RELEASE_VERSION}}', VERSION).replace('{{VERSION}}', VERSION.split('-')[0]))
        for name in [*LIBRARIES, 'neoism-update-helper']:
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
        # MSI tooling consumes DIST directly. Never merge into a stale browser tree.
        for child in (DIST / 'web').iterdir():
            if child.name not in ('app', 'native'):
                if child.is_dir() and not child.is_symlink():
                    shutil.rmtree(child)
                else:
                    child.unlink()
        shutil.copytree(bundle / 'web', DIST / 'web', dirs_exist_ok=True)
        (bundle / 'README.txt').write_text('Extract the entire folder, then run neoism.exe. Keep the DLLs, services, web, resources and licenses beside it.\n')
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
    if SYSTEM == 'windows':
        verify_windows_payload(installed)
    docs = installed.parent / 'Resources/resources/docs' if SYSTEM == 'darwin' else installed / 'resources/docs'
    run('node', ROOT / 'scripts/build-docs.mjs', '--verify', docs)
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
