#!/usr/bin/env python3
"""Bounded Linux-only RAM attribution; no builds, signals, or config changes.

Usage: python3 scripts/capture-memory-linux.py --output /tmp/neoism-memory.jsonl
Defaults: 15 minutes, 10-second interval, 16 MiB maximum. Output is private
(mode 0600), created exclusively; never overwrites an existing capture.
Records host pressure/swap counters and at most 128 processes per sample:
Neoism/Rust tools, descendants, ancestors, and the 20 largest host processes.
PSS is sampled for the 40 largest selected processes only (null means unsampled
or inaccessible, NOT zero). RSS is not additive across shared mappings.
No command lines, environment dumps, file contents, or conversation data are
saved. cwd/executable/cgroup paths are local metadata; treat captures as private.
Polling can miss short-lived jobs. PID + start_ticks identifies reused PIDs;
PPID/PGID, cwd, crate/package and target-dir identify workload ownership.
"""
from __future__ import annotations

import argparse
import datetime
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time

PROC = Path('/proc')


def text(path: Path) -> str:
    try:
        return path.read_text(errors='replace')
    except OSError:
        return ''


def link(path: Path) -> str | None:
    try:
        return os.readlink(path)
    except OSError:
        return None


def counters(path: Path, keys: set[str]) -> dict[str, int]:
    result = {}
    for line in text(path).splitlines():
        fields = line.replace(':', '').split()
        if len(fields) >= 2 and fields[0] in keys:
            try:
                result[fields[0]] = int(fields[1])
            except ValueError:
                pass
    return result


def attribution(path: Path) -> dict:
    # Only allowlisted build metadata; never emit arbitrary argv or env values.
    try:
        with (path / 'cmdline').open('rb') as f:
            args = f.read(65536).decode(errors='replace').split('\0')
    except OSError:
        return {}
    result = {}
    for flag in ('--crate-name', '-p', '--package', '--target', '--target-dir', '--jobs', '-j'):
        values = []
        for i, arg in enumerate(args):
            value = args[i + 1] if arg == flag and i + 1 < len(args) else (
                arg[len(flag) + 1:] if arg.startswith(flag + '=') else '')
            if value and re.fullmatch(r'[\w./+:-]{1,256}', value):
                values.append(value)
        if values:
            result[flag] = values[:8]
    result['roles'] = [a for a in args if a in {
        '--neoism-internal-workspace-daemon', '--neoism-internal-agent-server',
        '--neoism-notes-mcp', '--new-window',
        'check', 'build', 'test', 'clippy', '--all-targets', '--release', '--stdio'}]
    return result


def sample() -> dict:
    # One cheap process-table read; do not read every process's smaps/environ.
    raw = subprocess.check_output(
        ['ps', '-eo', 'pid=,ppid=,pgid=,rss=,comm='], text=True, timeout=5)
    rows = {}
    for line in raw.splitlines():
        fields = line.split(None, 4)
        if len(fields) == 5:
            pid, ppid, pgid, rss = map(int, fields[:4])
            rows[pid] = dict(pid=pid, ppid=ppid, pgid=pgid, rss_kib=rss, comm=fields[4])
    relevant = {p for p, r in rows.items() if re.search(
        r'neoism|rust-analy|rustc|cargo|rustfmt|clippy|rust-lld|^ld$|^mold$', r['comm'])}
    while True:
        expanded = relevant | {p for p, r in rows.items() if r['ppid'] in relevant}
        if expanded == relevant:
            break
        relevant = expanded
    ranked = sorted(rows, key=lambda p: rows[p]['rss_kib'], reverse=True)
    selected = relevant | set(ranked[:20])
    for pid in list(selected):
        seen = set()
        while pid in rows and pid not in seen:
            seen.add(pid)
            selected.add(pid)
            pid = rows[pid]['ppid']
    ordered = sorted(selected, key=lambda p: rows[p]['rss_kib'], reverse=True)
    records = []
    for i, pid in enumerate(ordered[:128]):
        path = PROC / str(pid)
        stat = text(path / 'stat').rsplit(')', 1)
        if len(stat) != 2:
            continue
        fields = stat[1].split()
        if len(fields) < 20:
            continue
        record = dict(rows[pid], start_ticks=int(fields[19]), state=fields[0],
                      cwd=link(path / 'cwd'), exe=link(path / 'exe'),
                      relevant=pid in relevant, attribution=attribution(path),
                      cgroup=text(path / 'cgroup')[:1024].strip(), pss_kib=None)
        record.update(counters(path / 'status', {
            'VmRSS', 'VmHWM', 'VmSwap', 'RssAnon', 'RssFile', 'Threads'}))
        if i < 40:
            smaps = counters(path / 'smaps_rollup', {'Pss', 'Private_Dirty', 'Private_Clean', 'SwapPss'})
            record['pss_kib'] = smaps.pop('Pss', None)
            record.update(smaps)
        # Discard a row if the process exited or the PID was reused mid-read.
        after = text(path / 'stat').rsplit(')', 1)
        if len(after) == 2 and len(after[1].split()) >= 20 and after[1].split()[19] == fields[19]:
            records.append(record)
    return dict(time=datetime.datetime.now().astimezone().isoformat(),
                monotonic=time.monotonic(),
                memory_kib=counters(PROC / 'meminfo', {
                    'MemTotal', 'MemAvailable', 'MemFree', 'Cached', 'SwapTotal',
                    'SwapFree', 'SwapCached', 'AnonPages', 'Slab', 'SReclaimable', 'Shmem'}),
                vmstat=counters(PROC / 'vmstat', {'pswpin', 'pswpout', 'pgmajfault', 'oom_kill'}),
                pressure=text(PROC / 'pressure/memory').strip(),
                process_count=len(rows), selected_count=len(selected),
                omitted=max(0, len(selected) - 128), processes=records)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--duration', type=int, default=900, help='seconds, 1..3600')
    parser.add_argument('--interval', type=int, default=10, help='seconds, 2..60')
    parser.add_argument('--max-mib', type=int, default=16, help='output cap, 1..64 MiB')
    args = parser.parse_args()
    if sys.platform != 'linux':
        parser.error('Linux /proc and procps ps are required')
    if not (1 <= args.duration <= 3600 and 2 <= args.interval <= 60 and 1 <= args.max_mib <= 64):
        parser.error('duration, interval, or max-mib outside bounded range')
    fd = os.open(args.output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    deadline = time.monotonic() + args.duration
    written = 0
    with os.fdopen(fd, 'w') as output:
        header = dict(schema=1, scope='Linux host', boot_id=text(PROC / 'sys/kernel/random/boot_id').strip(),
                      boot_time_unix=counters(PROC / 'stat', {'btime'}).get('btime'),
                      clock_ticks=os.sysconf('SC_CLK_TCK'), duration=args.duration, interval=args.interval)
        output.write(json.dumps(header) + '\n')
        written += len(json.dumps(header).encode()) + 1
        while time.monotonic() < deadline:
            started = time.monotonic()
            try:
                record = sample()
            except (OSError, subprocess.SubprocessError) as error:
                record = dict(time=datetime.datetime.now().astimezone().isoformat(), error=type(error).__name__)
            line = json.dumps(record, separators=(',', ':')) + '\n'
            size = len(line.encode())
            if written + size > args.max_mib * 1024 * 1024:
                break
            output.write(line)
            output.flush()
            written += size
            time.sleep(max(0, min(deadline - time.monotonic(), args.interval - (time.monotonic() - started))))
    print(f'Capture saved: {args.output} ({written} bytes)')


if __name__ == '__main__':
    main()
