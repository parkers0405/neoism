#!/usr/bin/env python3
"""Retain Cargo freshness only for source bytes proven identical to every restored cache."""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess


def git(root, *args):
    return subprocess.check_output(['git', '-C', str(root), *args])


def restore(root, commits):
    trees = []
    for commit in commits:
        tree = {}
        for record in git(root, 'ls-tree', '-rz', '--full-tree', commit).split(b'\0'):
            if not record:
                continue
            metadata, name = record.split(b'\t', 1)
            mode, kind, digest = metadata.split()
            if kind == b'blob' and mode in (b'100644', b'100755'):
                tree[os.fsdecode(name)] = digest.decode()
        trees.append(tree)
    if not trees:
        return 0
    restored = 0
    for name, expected in trees[0].items():
        if any(tree.get(name) != expected for tree in trees[1:]):
            continue
        path = root / name
        if path.is_symlink() or not path.is_file():
            continue
        stat = path.stat()
        digest = hashlib.sha1(f'blob {stat.st_size}\0'.encode())
        with path.open('rb') as source:
            while chunk := source.read(1024 * 1024):
                digest.update(chunk)
        if digest.hexdigest() != expected:
            continue
        # This is safe even for a changed checkout: changed bytes never reach
        # this point. Cargo still checks compiler/options/dependency fingerprints.
        os.utime(path, ns=(stat.st_atime_ns, 946684800_000000000))
        restored += 1
    return restored


def main():
    combined = os.environ.get('COMBINED_CACHE_KEY', '')
    keys = [combined] if combined else [os.environ.get('SERVICE_CACHE_KEY', ''), os.environ.get('NATIVE_CACHE_KEY', '')]
    keys = list(dict.fromkeys(key for key in keys if key))
    if not keys:
        print('No compiled workspace cache to reuse.')
        return
    repository = os.environ['GITHUB_REPOSITORY']
    root = Path(__file__).resolve().parent.parent
    commits = []
    for key in keys:
        match = re.fullmatch(r'gpuix-(?:native|services)-.+-(\d+)-\d+', key)
        if not match:
            raise ValueError(f'Unrecognized compilation cache key: {key}')
        run = json.loads(subprocess.check_output(['gh', 'api', f'repos/{repository}/actions/runs/{match[1]}']))
        commit = run['head_sha']
        if run['head_repository']['full_name'] != repository or not re.fullmatch(r'[0-9a-f]{40}', commit):
            raise ValueError('Compilation cache source does not belong to this repository')
        git(root, 'fetch', '--no-tags', '--depth=1', 'origin', commit)
        commits.append(commit)
    count = restore(root, commits)
    print(f'Restored Cargo source timestamps for {count} files identical to all cached source commits: {", ".join(commits)}')


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        # Cache reuse is optional. A lookup/network failure must only make the
        # build slower, never bypass compilation or prevent a release.
        print(f'::warning::Source timestamp reuse unavailable; Cargo will rebuild normally: {error}')
