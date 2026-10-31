#!/usr/bin/env python3
"""Check native dependency MSRVs before the long service/display compilation."""
import argparse
import json
from pathlib import Path
import re
import subprocess

root = Path(__file__).resolve().parent.parent
frontend = root / 'neoism-frontend/gpuix'
parser = argparse.ArgumentParser()
parser.add_argument('--target', help='Validate another platform without compiling it')
args = parser.parse_args()
compiler = subprocess.check_output(['rustc', '-vV'], cwd=frontend, text=True)
version = tuple(map(int, re.search(r'^release: (\d+)\.(\d+)\.(\d+)', compiler, re.M).groups()))
target = args.target or re.search(r'^host: (.+)$', compiler, re.M)[1]
metadata = json.loads(subprocess.check_output([
    'cargo', 'metadata', '--locked', '--format-version', '1',
    '--manifest-path', 'native/Cargo.toml', '--filter-platform', target,
], cwd=frontend, text=True))
nodes = {node['id']: node for node in metadata['resolve']['nodes']}
pending = [metadata['resolve']['root']]
used = set()
while pending:
    identity = pending.pop()
    if identity in used:
        continue
    used.add(identity)
    for dependency in nodes[identity]['deps']:
        if any(kind['kind'] != 'dev' for kind in dependency['dep_kinds']):
            pending.append(dependency['pkg'])
incompatible = []
for package in metadata['packages']:
    if package['id'] not in used or not package.get('rust_version'):
        continue
    minimum = tuple(int(part) for part in package['rust_version'].split('.'))
    minimum = (*minimum, *(0 for _ in range(3 - len(minimum))))
    if minimum > version:
        incompatible.append(f"{package['name']} {package['version']} requires Rust {package['rust_version']}")
if incompatible:
    raise SystemExit(f'Native {target} dependencies exceed Rust {".".join(map(str, version))}:\n' + '\n'.join(incompatible))
print(f'Native {target} dependency toolchain check passed with Rust {".".join(map(str, version))}.')
