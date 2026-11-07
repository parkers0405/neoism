#!/usr/bin/env python3
"""Release gate: validate GUI subsystem and freshly built service provenance."""
import hashlib
from pathlib import Path
import struct


def verify_windows_bundle(bundle: Path, services: Path):
    executable = (bundle / 'neoism.exe').read_bytes()
    try:
        if executable[:2] != b'MZ':
            raise ValueError('missing DOS signature')
        offset = struct.unpack_from('<I', executable, 0x3c)[0]
        if executable[offset:offset + 4] != b'PE\0\0':
            raise ValueError('missing PE signature')
        if struct.unpack_from('<H', executable, offset + 24)[0] not in (0x10b, 0x20b):
            raise ValueError('unsupported optional header')
        if struct.unpack_from('<H', executable, offset + 92)[0] != 2:
            raise ValueError('not a GUI executable')
    except (ValueError, struct.error) as error:
        raise ValueError('neoism.exe must have Windows GUI PE subsystem 2; refusing a console launcher') from error
    hashes = {}
    for name in ('neoism-workspace-daemon.exe', 'neoism-agent.exe', 'neoism-agent-lua-runner.exe'):
        shipped = hashlib.sha256((bundle / name).read_bytes()).hexdigest()
        built = hashlib.sha256((services / name).read_bytes()).hexdigest()
        if shipped != built:
            raise ValueError(f'{name} differs from this release build ({services}); refusing stale packaged service')
        hashes[name] = shipped
    return hashes


if __name__ == '__main__':
    import argparse
    import json
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('bundle', type=Path)
    parser.add_argument('services', type=Path)
    args = parser.parse_args()
    print(json.dumps(verify_windows_bundle(args.bundle, args.services), indent=2))