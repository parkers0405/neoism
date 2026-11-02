#!/usr/bin/env python3
"""Repair a published Linux archive without rebuilding or changing existing files."""
import hashlib
import io
import json
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile

source, destination = map(Path, sys.argv[1:])
assert source.name == destination.name == 'neoism-linux-x86_64.tar.gz'
assert not destination.exists(), 'Refusing to overwrite an existing repair'
digest = lambda path: hashlib.file_digest(path.open('rb'), 'sha256').hexdigest()
original_digest = digest(source)
assert original_digest == Path(str(source) + '.sha256').read_text().split()[0]
root = 'neoism-linux-x86_64'
marker = f'{root}/web/agent-gui/index.html'
content = b'<!-- Neoism native runtime resources; no browser UI. -->\n'

def manifest(path):
    result = {}
    with tarfile.open(path) as archive:
        for entry in archive:
            assert entry.name not in result, f'Duplicate member: {entry.name}'
            result[entry.name] = (entry.type, entry.mode, entry.linkname,
                hashlib.file_digest(archive.extractfile(entry), 'sha256').hexdigest() if entry.isfile() else None)
    return result

before = manifest(source)
assert not any(name.startswith(f'{root}/web/agent-gui') for name in before)
destination.parent.mkdir(parents=True, exist_ok=True)
with tarfile.open(source) as original, tarfile.open(destination, 'w:gz') as repaired:
    for entry in original:
        repaired.addfile(entry, original.extractfile(entry) if entry.isfile() else None)
    directory = tarfile.TarInfo(f'{root}/web/agent-gui')
    directory.type = tarfile.DIRTYPE
    directory.mode = 0o755
    repaired.addfile(directory)
    entry = tarfile.TarInfo(marker)
    entry.mode = 0o644
    entry.size = len(content)
    repaired.addfile(entry, io.BytesIO(content))
after = manifest(destination)
assert set(after) - set(before) == {f'{root}/web/agent-gui', marker}
assert all(after[name] == value for name, value in before.items()), 'Existing payload changed'
with tempfile.TemporaryDirectory(prefix='neoism-repair-verify-') as temporary:
    with tarfile.open(destination) as archive:
        archive.extractall(temporary, filter='data')
    bundle = Path(temporary) / root
    for name in ['neoism', 'neoism-agent', 'neoism-agent-lua-runner', 'neoism-workspace-daemon',
                 'web/index.html', 'web/agent-gui/index.html', 'web/app/manifest.json']:
        assert (bundle / name).is_file(), f'Old updater requirement missing: {name}'
    assert (bundle / 'libneoism_gpuix_core.so').read_bytes() == (bundle / 'web/native/libneoism_gpuix_core.so').read_bytes()
    subprocess.run([bundle / 'neoism', '--neoism-verify-runtime'], check=True)
    subprocess.run([bundle / 'neoism', '--version'], check=True)
repaired_digest = digest(destination)
Path(str(destination) + '.sha256').write_text(f'{repaired_digest}  {destination.name}\n')
receipt = destination.parent / 'neoism-linux-x86_64.packaging-repair.json'
receipt.write_text(json.dumps({'original_sha256': original_digest, 'repaired_sha256': repaired_digest,
    'added': [marker], 'existing_members_verified_unchanged': len(before),
    'reason': 'Inert marker required by deployed .2 updater; no browser UI restored'}, indent=2) + '\n')
print(receipt.read_text())