"""Recover symbol hints from an existing perf user-stack capture locally.

This is a stack scan, NOT a validated unwind: stale return addresses may remain.
Only aggregate function names are printed; captured user memory stays private.
"""
import argparse
import bisect
import collections
import re
import struct
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument("capture")
parser.add_argument("library")
parser.add_argument("--base", type=lambda value: int(value, 0), required=True)
parser.add_argument("--pid", type=int, required=True)
parser.add_argument("--perf", required=True)
args = parser.parse_args()
symbols = []
for line in subprocess.check_output(["nm", "-n", "-S", "-C", "--defined-only", args.library], text=True).splitlines():
    parts = line.split(maxsplit=3)
    if len(parts) == 4 and parts[2] in ("t", "T"):
        symbols.append((int(parts[0], 16), int(parts[1], 16), parts[3]))
symbols.sort()
starts = [entry[0] for entry in symbols]
counts = collections.Counter()
nearest = collections.Counter()
samples = matched = 0
raw = bytearray()
stack = None
selected = False


def consume():
    global samples, matched
    if not selected or stack is None:
        return
    samples += 1
    size, offset = stack
    found = []
    for position in range(offset, min(offset + size, len(raw), offset + 8192) - 7, 8):
        address = struct.unpack_from("<Q", raw, position)[0] - args.base
        index = bisect.bisect_right(starts, address - 1) - 1
        if index < 0:
            continue
        start, length, name = symbols[index]
        if start <= address - 1 < start + length and name.startswith(("neoism_gpuix_core::", "neoism_ui::")):
            found.append(name)
    if found:
        matched += 1
        nearest[found[0]] += 1
        counts.update(set(found))


process = subprocess.Popen([args.perf, "script", "-D", "-i", args.capture], stdout=subprocess.PIPE, text=True)
for line in process.stdout:
    if re.match(r"0x[0-9a-f]+@", line):
        consume()
        raw = bytearray()
        stack = None
        selected = False
    row = re.match(r"\.  [0-9a-f]+:  ((?:[0-9a-f]{2} ){1,16})", line)
    if row:
        raw.extend(bytes.fromhex(row[1]))
    info = re.search(r"ustack: size (\d+), offset (0x[0-9a-f]+)", line)
    if info:
        stack = (int(info[1]), int(info[2], 16))
    if line.strip() == f"... thread: neoism:{args.pid}":
        selected = True
consume()
if process.wait():
    raise SystemExit("perf decoding failed")
print({"main_samples": samples, "samples_with_native_hints": matched, "caution": "heuristic stack scan, not validated unwind"})
print("Nearest native caller hints:")
for name, count in nearest.most_common(20):
    print(count, name[:220])
print("All native caller hints:")
for name, count in counts.most_common(20):
    print(count, name[:220])