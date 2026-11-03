#!/usr/bin/env python3
"""Bounded, read-only Linux process profiler; stdout is a single sanitized JSON object.

Requires procps ps and sysstat pidstat. No /proc, environment, command lines, or
pidstat command labels are included in the report. Command lines are used only
transiently to classify roles. Sampling cannot reliably capture short-lived jobs.
"""

import argparse
from collections import defaultdict
from dataclasses import dataclass
import json
import math
import os
import re
import shlex
import shutil
import subprocess
import sys
import threading
import time


MAX_IDENTITIES = 32768
MAX_PROCESSES = 200
MAX_PSS = 1024
MAX_BYTES = 32 * 1024 * 1024
MAX_ROWS = 32768
PS_FIELDS = "pid=,ppid=,lstart=,nlwp=,rss=,comm=,args="
NEOISM_ROOT_ROLES = {"frontend", "workspace_server", "agent_server"}
RATES = {
    "cpu_percent": ("task-cpu-load", "cpu"),
    "scheduler_wait_percent": ("task-cpu-load", "wait"),
    "read_kib_s": ("io", "kB_rd/s"),
    "write_kib_s": ("io", "kB_wr/s"),
    "major_faults_s": ("task-memory", "majflt/s"),
    "voluntary_switches_s": ("context-switch", "cswch/s"),
    "involuntary_switches_s": ("context-switch", "nvcswch/s"),
}


def number(value):
    try:
        result = float(value)
        return result if math.isfinite(result) and result >= 0 else None
    except (TypeError, ValueError):
        return None


def classify(comm, args):
    """Never return user-controlled text. Generic Node is not a candidate."""
    try:
        words = shlex.split(args)
    except ValueError:
        words = args.split()
    executable = words[0] if words else comm
    base = os.path.basename(executable)
    # ps comm is truncated to 15 bytes on some kernels; prefer executable.
    name = base.lower()
    tokens = set(words[1:])
    if name in {"neoism-agent-server", "neoism-agent"} or (
        name == "neoism" and tokens & {"--agent-server", "--neoism-internal-agent-server"}
    ):
        return "agent_server"
    if name in {"neoism-workspace-daemon", "neoism-workspace-server", "neoism-daemon", "neoism-server"} or (
        name == "neoism" and tokens & {"--daemon", "--server", "daemon", "--neoism-internal-workspace-daemon"}
    ):
        return "workspace_server"
    if name in {"neoism", "neoism-gpuix", "neoism-preview"}:
        return "frontend"
    if "rust-analyzer" in name:
        return "rust_analyzer"
    if name in {"cargo", "rustc", "rustdoc"}:
        return name
    # Inspect only the entrypoint, not e.g. tsserver's --globalPlugins argument
    # containing 'typescript-language-server'. Neither script args nor project
    # paths are ownership evidence.
    entrypoint = name
    source_entrypoint = None
    preloads = []
    if name in {"node", "nodejs", "bun", "deno"}:
        skip = False
        preload_next = False
        for word in words[1:]:
            if preload_next:
                preloads.append(word)
                preload_next = False
                continue
            if skip:
                skip = False
                continue
            if name == "bun" and word == "--preload":
                preload_next = True
                continue
            if name == "bun" and word.startswith("--preload="):
                preloads.append(word.partition("=")[2])
                continue
            if word in {"-r", "--require", "--loader", "--import"}:
                skip = True
                continue
            if word in {"-e", "--eval", "-p", "--print"}:
                break
            if not word.startswith("-"):
                source_entrypoint = word
                entrypoint = word.lower()
                break
    # The source GPUIX frontend uses Bun, but ordinary Bun/Node main.tsx
    # programs are not Neoism roots. Require both distinctive launch paths,
    # parsed as preload option + actual entrypoint, not later script arguments.
    if name == "bun" and source_entrypoint is not None and re.search(r"(?:^|/)src/main\.tsx$", source_entrypoint) and any(
        re.search(r"(?:^|/)scripts/display-binding-preload\.ts$", path) for path in preloads
    ):
        return "frontend"
    if re.search(r"(?:^|/)tsserver(?:\.js)?$", entrypoint):
        return "tsserver"
    if re.search(r"(?:^|/)typingsinstaller(?:\.js)?$", entrypoint):
        return "typings_installer"
    if re.search(r"(?:^|/)typescript-language-server(?:[./]|$)", entrypoint):
        return "typescript_language_server"
    if re.search(r"(?:^|/)yaml-language-server(?:[./]|$)", entrypoint):
        return "yaml_language_server"
    if re.search(r"(?:^|/)(?:vscode-json-languageserver|vscode-css-languageserver|eslint-language-server|pyright-langserver|basedpyright-langserver|gopls|clangd|svelte-language-server|taplo)(?:[./]|$)", entrypoint):
        return "other_language_server"
    if name in {"node", "nodejs", "bun", "deno"}:
        return "node_runtime"
    if name in {"cc", "gcc", "g++", "clang", "clang++", "ld", "lld", "cc1", "cc1plus", "make", "cmake", "ninja", "sccache", "rustfmt", "clippy-driver", "git", "gh", "rg", "fff-search"}:
        return "other_tool"
    return "other"


@dataclass
class Process:
    pid: int
    ppid: int
    start: str
    threads: int
    rss: int
    role: str

    @property
    def key(self):
        return (self.pid, self.start)


def parse_ps(text):
    processes = []
    for line in text.splitlines()[:MAX_ROWS]:
        fields = line.split(None, 10)
        if len(fields) < 10:
            continue
        try:
            processes.append(Process(
                int(fields[0]), int(fields[1]), " ".join(fields[2:7]),
                int(fields[7]), int(fields[8]),
                classify(fields[9], fields[10] if len(fields) > 10 else fields[9]),
            ))
        except ValueError:
            continue
    return processes


class Collector:
    """Drain immediately in a thread: ps sampling never blocks pidstat's pipe."""

    def __init__(self, command):
        self.process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                        env=dict(os.environ, LC_ALL="C", S_TIME_FORMAT="ISO"))
        self.data = bytearray()
        self.truncated = False
        self.thread = threading.Thread(target=self._drain, daemon=True)
        self.thread.start()

    def _drain(self):
        while True:
            chunk = self.process.stdout.read(65536)
            if not chunk:
                break
            available = MAX_BYTES - len(self.data)
            self.data.extend(chunk[:available])
            if len(chunk) > available:
                self.truncated = True
        self.process.stdout.close()

    def finish(self, timeout):
        timed_out = False
        try:
            self.process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            timed_out = True
            self.process.kill()
            self.process.wait()
        self.thread.join(timeout=2)
        return bytes(self.data).decode("utf-8", errors="replace"), self.process.returncode, timed_out


def capture(command, timeout=3):
    collector = Collector(command)
    text, code, timed_out = collector.finish(timeout)
    return text, code, timed_out or collector.truncated


class History:
    def __init__(self, root_pid, profiler_pid):
        self.root_pid = root_pid
        self.profiler_pid = profiler_pid
        self.root_key = None
        self.records = {}
        self.latest = {}
        self.parents = {}
        self.snapshots = []
        self.reused = set()
        self.omitted = 0

    def add(self, offset, processes):
        current = {}
        for process in processes:
            key = process.key
            if key not in self.records and len(self.records) >= MAX_IDENTITIES:
                self.omitted += 1
                continue
            previous = self.latest.get(process.pid)
            if previous is not None and previous != key:
                self.reused.add(process.pid)
            self.records[key] = process
            self.latest[process.pid] = key
            current[process.pid] = key
            if self.root_key is None and process.pid == self.root_pid:
                self.root_key = key
        for key in current.values():
            process = self.records[key]
            parent = current.get(process.ppid) or self.latest.get(process.ppid)
            # Keep original ancestry when an exited parent causes reparenting.
            if key not in self.parents or self.parents[key] is None:
                self.parents[key] = parent
        self.snapshots.append((offset, current))

    def ownership(self, key):
        chain = []
        seen = set()
        cursor = key
        while cursor is not None and cursor not in seen:
            seen.add(cursor)
            chain.append(cursor)
            cursor = self.parents.get(cursor)
        if any(item[0] == self.profiler_pid for item in chain):
            return None
        if self.root_key in chain:
            return "active_frontend", self.root_key[0]
        # Use the outermost observed Neoism root so a preview frontend's
        # servers stay in its tree. An orphan standalone server is itself a
        # meaningful root, including for otherwise generic Node descendants.
        for item in reversed(chain):
            if self.records[item].role in NEOISM_ROOT_ROLES:
                return "other_neoism_tree", item[0]
        if self.records[key].role not in {"other", "node_runtime"}:
            return "unattached_candidate", None
        return None

    def at(self, pid, offset):
        # pidstat has no lstart: do not guess which incarnation owned its rates.
        if pid in self.reused:
            return None
        for when, current in reversed(self.snapshots):
            if when <= offset + 0.25 and pid in current:
                return current[pid]
        return None


def pss_endpoint(history, ps):
    current = history.snapshots[-1][1]
    keys = [key for key in current.values() if history.ownership(key)]
    values = {}
    failed = False
    deadline = time.monotonic() + 3
    targeted = 0
    for start in range(0, min(len(keys), MAX_PSS), 256):
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            failed = True
            break
        batch = keys[start:start + 256]
        targeted += len(batch)
        text, code, truncated = capture([ps, "-p", ",".join(str(key[0]) for key in batch), "-o", "pid=,pss="], timeout=remaining)
        failed |= code != 0 or truncated
        lookup = {key[0]: key for key in batch}
        for line in text.splitlines():
            parts = line.split()
            if len(parts) == 2 and parts[0].isdigit() and int(parts[0]) in lookup:
                value = number(parts[1])
                if value is not None:
                    values[lookup[int(parts[0])]] = value
    return values, {"targeted": targeted, "known": len(values),
                    "omitted": len(keys) - targeted, "query_failed_or_unsupported": failed}


def parse_pidstat(text, count):
    try:
        statistics = json.loads(text)["sysstat"]["hosts"][0]["statistics"]
        if not isinstance(statistics, list):
            raise ValueError()
        return statistics[:count], len(statistics) > count
    except (ValueError, KeyError, IndexError, TypeError):
        return [], True


def reduce_rates(slots, count):
    known = [value for value in slots.values() if value is not None]
    return {"period_mean": round(sum(known) / count, 4) if known and count else None,
            "peak": round(max(known), 4) if known else None,
            "known_intervals": len(known)}


def summarize(history, statistics, interval, count, pss_start, pss_end):
    selected = {key: history.ownership(key) for key in history.records if history.ownership(key)}
    metrics = {key: {name: {} for name in RATES} for key in selected}
    memory = {key: [] for key in selected}
    group_slots = defaultdict(lambda: {name: defaultdict(float) for name in RATES})
    group_memory = defaultdict(lambda: defaultdict(float))
    group_threads = defaultdict(list)
    threads = defaultdict(list)
    missing = defaultdict(lambda: defaultdict(int))
    rows_omitted = 0

    def group(key):
        ownership, root = selected[key]
        return ownership, root, history.records[key].role

    for index, statistic in enumerate(statistics):
        if not isinstance(statistic, dict):
            continue
        for name, (section, field) in RATES.items():
            rows = statistic.get(section, [])
            if not isinstance(rows, list):
                continue
            rows_omitted += max(0, len(rows) - MAX_ROWS)
            for row in rows[:MAX_ROWS]:
                if not isinstance(row, dict):
                    continue
                pid = number(row.get("PID"))
                key = history.at(int(pid), (index + 1) * interval) if pid is not None else None
                if key not in selected:
                    continue
                value = number(row.get(field))
                if value is not None:
                    metrics[key][name][index] = value
        rows = statistic.get("task-memory", [])
        for row in rows[:MAX_ROWS] if isinstance(rows, list) else []:
            if not isinstance(row, dict):
                continue
            pid = number(row.get("PID"))
            key = history.at(int(pid), (index + 1) * interval) if pid is not None else None
            rss = number(row.get("RSS"))
            if key in selected and rss is not None:
                memory[key].append(rss)
                group_memory[group(key)][("pidstat", index)] += rss

    for key, names in metrics.items():
        for name, slots in names.items():
            for index, value in slots.items():
                group_slots[group(key)][name][index] += value
            missing[group(key)][name] += count - len(slots)

    # Snapshots retain scalar measurements separately, since records are latest.
    for snapshot in getattr(history, "measurements", []):
        group_rss = defaultdict(float)
        group_nlwp = defaultdict(int)
        for key, rss, nlwp in snapshot:
            if key in selected:
                memory[key].append(rss)
                threads[key].append(nlwp)
                group_rss[group(key)] += rss
                group_nlwp[group(key)] += nlwp
        for label, rss in group_rss.items():
            group_memory[label][("ps", len(group_threads[label]))] = rss
        for label, nlwp in group_nlwp.items():
            group_threads[label].append(nlwp)

    def peak(values):
        return max(values) if values else None

    def endpoint(keys, values):
        known = [values[key] for key in keys if key in values and key[0] not in history.reused]
        return {"known_sum": sum(known) if known else None, "known_processes": len(known),
                "unknown_processes": len(keys) - len(known)}

    processes = []
    for key, (ownership, root) in selected.items():
        record = history.records[key]
        processes.append({"pid": record.pid, "identity": record.start, "role": record.role,
                          "ownership": ownership, "tree_root_pid": root,
                          "rates": {name: reduce_rates(slots, count) for name, slots in metrics[key].items()},
                          "rss_peak_kib": peak(memory[key]), "threads_peak": peak(threads[key]),
                          "pss_start_kib": pss_start.get(key) if record.pid not in history.reused else None,
                          "pss_end_kib": pss_end.get(key) if record.pid not in history.reused else None})
    processes.sort(key=lambda row: (-(row["rates"]["cpu_percent"]["period_mean"] or 0), -(row["rss_peak_kib"] or 0), row["pid"]))
    groups = []
    labels = sorted({group(key) for key in selected}, key=lambda label: (label[0], label[1] or 0, label[2]))
    for label in labels:
        keys = [key for key in selected if group(key) == label]
        groups.append({"ownership": label[0], "tree_root_pid": label[1], "role": label[2],
                       "process_identities": len(keys),
                       "rates": {name: dict(reduce_rates(group_slots[label][name], count),
                                            unknown_process_intervals=missing[label][name]) for name in RATES},
                       "rss_peak_kib": peak(list(group_memory[label].values())),
                       "threads_peak": peak(group_threads[label]),
                       "pss_start_kib": endpoint(keys, pss_start), "pss_end_kib": endpoint(keys, pss_end)})
    # Number of groups is bounded too; group metrics include all selected records.
    return {"processes": processes[:MAX_PROCESSES], "groups": groups[:MAX_PROCESSES],
            "omissions": {"process_output": max(0, len(processes) - MAX_PROCESSES),
                          "group_output": max(0, len(groups) - MAX_PROCESSES),
                          "identity_observations": history.omitted, "pidstat_rows": rows_omitted},
            "pid_reuse_detected": sorted(history.reused)[:MAX_PROCESSES],
            "pid_reuse_count": len(history.reused), "selected_process_identities": len(selected)}


def bounded_int(low, high):
    def parse(value):
        result = int(value)
        if not low <= result <= high:
            raise argparse.ArgumentTypeError("must be between %d and %d" % (low, high))
        return result
    return parse


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root-pid", required=True, type=bounded_int(1, 2**31 - 1))
    parser.add_argument("--duration", default=30, type=bounded_int(1, 120))
    parser.add_argument("--interval", default=2, type=bounded_int(1, 120))
    parser.add_argument("--pidstat", help="pidstat executable name or path (default: pidstat on PATH)")
    args = parser.parse_args(argv)
    if args.interval > args.duration:
        parser.error("--interval must not exceed --duration")
    ps = shutil.which("ps")
    pidstat = shutil.which(args.pidstat or "pidstat")
    if pidstat is None:
        print(json.dumps({"schema_version": 1, "error": "pidstat_unavailable",
                          "message": "pidstat was not found or is not executable; use --pidstat /path/to/pidstat to select an existing sysstat executable, or make pidstat available on PATH."}))
        return 1
    history = History(args.root_pid, os.getpid())
    history.measurements = []
    flags = set()

    def snapshot(offset):
        text, code, truncated = capture([ps, "-e", "-ww", "-o", PS_FIELDS])
        if code or truncated:
            flags.add("ps_failed_or_truncated")
        if len(text.splitlines()) > MAX_ROWS:
            flags.add("ps_rows_truncated")
        rows = parse_ps(text)
        history.add(offset, rows)
        history.measurements.append([(row.key, row.rss, row.threads) for row in rows if row.key in history.records])

    collector = None
    try:
        if ps is None:
            raise OSError()
        snapshot(0)
        if history.root_key is None:
            print(json.dumps({"schema_version": 1, "error": "root_pid_not_observed"}))
            return 1
        start_pss, start_status = pss_endpoint(history, ps)
        count = args.duration // args.interval
        started = time.monotonic()
        collector = Collector([pidstat, "-u", "-r", "-d", "-w", "-o", "JSON", "-p", "ALL", str(args.interval), str(count)])
        for index in range(1, count + 1):
            if time.monotonic() > started + count * args.interval + 0.5:
                flags.add("ownership_sampling_deadline_exceeded")
                break
            time.sleep(max(0, started + index * args.interval - time.monotonic()))
            snapshot(time.monotonic() - started)
        text, code, timed_out = collector.finish(5)
        elapsed = time.monotonic() - started
        if code or timed_out or collector.truncated:
            flags.add("pidstat_failed_timed_out_or_truncated")
        statistics, invalid = parse_pidstat(text, count)
        if invalid:
            flags.add("pidstat_invalid_or_truncated_json")
        if len(statistics) != count:
            flags.add("pidstat_missing_intervals")
        end_pss, end_status = pss_endpoint(history, ps)
        report = summarize(history, statistics, args.interval, count, start_pss, end_pss)
        report.update({"schema_version": 1, "root_pid": args.root_pid,
                       "sampling": {"requested_seconds": args.duration, "period_seconds": count * args.interval,
                                    "wall_seconds": round(elapsed, 3), "interval_seconds": args.interval,
                                    "expected_intervals": count, "received_intervals": len(statistics),
                                    "ownership_snapshots": len(history.snapshots), "observed_identities": len(history.records)},
                       "pss_queries": {"start": start_status, "end": end_status}, "flags": sorted(flags),
                       "semantics": {
                           "cpu": "percent of one logical CPU; period_mean=sum(interval rates)/expected intervals (not lifetime ps CPU)",
                           "missing": "null means unknown, not zero; partial period means are known contributions only, not complete estimates; inspect known_intervals and unknown_process_intervals",
                           "groups": "ownership + tree root + role; peaks sum simultaneous known samples, not individual peaks",
                            "ownership": "observed ancestry only; active frontend takes priority, otherwise outermost frontend/workspace/agent server roots define other Neoism trees; unattached_candidate is NOT proof of Neoism ownership; generic unattached Node excluded",
                           "memory": "KiB; RSS includes shared pages and group RSS double-counts them; PSS endpoints are non-atomic best effort; unknown endpoints stay null",
                           "limitations": "periodic ps/pidstat misses brief jobs and unobserved ancestry; lstart detects some PID reuse (one-second resolution); reused-PID rates withheld; no reliable brief-job capture claim",
                           "bounds": "120 seconds, 121 ownership samples, 32768 identities/rows, 32MiB per subprocess, 1024 PSS targets, 200 processes/groups output; ranked CPU then RSS",
                       }})
        print(json.dumps(report, separators=(",", ":"), allow_nan=False))
        return 0 if statistics else 1
    except (OSError, subprocess.SubprocessError):
        print(json.dumps({"schema_version": 1, "error": "process_tool_unavailable_or_failed"}))
        return 1
    finally:
        if collector is not None and collector.process.poll() is None:
            collector.process.kill()
            collector.finish(2)


if __name__ == "__main__":
    sys.exit(main())