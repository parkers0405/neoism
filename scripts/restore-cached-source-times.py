#!/usr/bin/env python3
"""Preserve original source freshness, including unfinished work in failed builds."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent
MANIFESTS = {
    'NATIVE': Path('target/gpuix-core-cargo/.neoism-source-times.json'),
    'SERVICE': Path('target/release/.neoism-source-times.json'),
}


def digest(path):
    with path.open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def snapshot(root):
    files = {}
    for raw in subprocess.check_output(['git', '-C', str(root), 'ls-files', '-z']).split(b'\0'):
        if not raw:
            continue
        name = os.fsdecode(raw)
        path = root / name
        if path.is_file() and not path.is_symlink():
            files[name] = {'sha256': digest(path), 'mtime_ns': path.stat().st_mtime_ns}
    return files


def restore(root, snapshots):
    if not snapshots:
        return 0
    restored = 0
    for name, entry in snapshots[0].items():
        entries = [files.get(name) for files in snapshots]
        if any(not other or other['sha256'] != entry['sha256'] for other in entries):
            continue
        relative = Path(name)
        if relative.is_absolute() or relative.drive or '..' in relative.parts:
            continue
        path = root / relative
        if not path.is_file() or path.is_symlink() or digest(path) != entry['sha256']:
            continue
        # Never manufacture an old timestamp. A failed build may have cached
        # old objects beside NEW source: its original mtime must stay newer
        # than those objects so Cargo still recompiles the unfinished work.
        timestamp = max(other['mtime_ns'] for other in entries)
        os.utime(path, ns=(path.stat().st_atime_ns, timestamp))
        restored += 1
    return restored


def main():
    if '--save' in sys.argv:
        files = snapshot(ROOT)
        for kind, relative in MANIFESTS.items():
            key = os.environ.get(f'{kind}_CACHE_SAVE_KEY')
            if not key:
                continue
            path = ROOT / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(json.dumps({'version': 1, 'key': key, 'files': files}), encoding='utf-8')
        print(f'Saved original timestamps and content hashes for {len(files)} source files.')
        return
    # Transitional caches predate the source snapshots. Inferring source
    # freshness from their run's HEAD is unsafe after a partially failed build.
    if os.environ.get('COMBINED_CACHE_KEY'):
        print('Legacy combined cache has no source snapshot; Cargo will rebuild normally.')
        return
    snapshots = []
    for kind, relative in MANIFESTS.items():
        key = os.environ.get(f'{kind}_CACHE_KEY')
        path = ROOT / relative
        if not key or not path.is_file():
            print('Source snapshot unavailable for a restored cache; Cargo will rebuild normally.')
            return
        manifest = json.loads(path.read_text(encoding='utf-8'))
        if manifest.get('version') != 1 or manifest.get('key') != key:
            raise ValueError('Source snapshot does not belong to the restored cache')
        snapshots.append(manifest['files'])
    print(f'Restored original source timestamps for {restore(ROOT, snapshots)} unchanged files.')


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError, subprocess.CalledProcessError) as error:
        print(f'::warning::Source timestamp reuse unavailable; Cargo will rebuild normally: {error}')
