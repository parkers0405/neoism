#!/usr/bin/env python3
"""Offline-first, dependency-free coding-agent evaluation pilot."""

import argparse
import base64
from collections import defaultdict
from datetime import datetime, timezone
import difflib
import hashlib
import json
import math
import os
from pathlib import Path, PurePosixPath
import random
import shutil
import statistics
import subprocess
import sys
import tempfile
import threading
import time
import uuid

import saved_auth

ROOT = Path(__file__).resolve().parent
DEFAULT_IMAGE = "neoism-eval-lab:local"
ARMS = {
    "neoism-sol": {"adapter": "neoism", "model": "gpt-6.1-sol", "credential": "OPENAI_API_KEY"},
    "claude-sonnet": {"adapter": "claude", "model": "claude-sonnet-5-5", "credential": "ANTHROPIC_API_KEY"},
    "claude-opus": {"adapter": "claude", "model": "claude-opus-5-5", "credential": "ANTHROPIC_API_KEY"},
}


def write_json(path, value):
    temporary = path.with_name(path.name + ".tmp")
    with temporary.open("w") as output:
        json.dump(value, output, indent=2, sort_keys=True)
        output.write("\n")
        output.flush()
        os.fsync(output.fileno())
    os.replace(temporary, path)
    directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)


def command(args, **kwargs):
    return subprocess.check_output(args, text=True, **kwargs).strip()


def image_id(image):
    return command(["docker", "image", "inspect", image, "--format", "{{.Id}}"])


def finite_positive(value):
    result = float(value)
    if not math.isfinite(result) or result <= 0:
        raise argparse.ArgumentTypeError("must be positive and finite")
    return result


def positive_integer(value):
    result = int(value)
    if result <= 0:
        raise argparse.ArgumentTypeError("must be positive")
    return result


def load_tasks(selected=None, suite="smoke"):
    paths = sorted((ROOT / "tasks").glob("*/task.json"))
    tasks = []
    for path in paths:
        task = json.loads(path.read_text())
        if task["id"] != path.parent.name:
            raise ValueError("Task ID must match directory: " + str(path))
        task.setdefault("suite", "smoke")
        if (suite != "all" and task["suite"] != suite) or (selected and task["id"] not in selected):
            continue
        for key in ("timeout_seconds", "verifier_timeout_seconds"):
            if type(task[key]) is not int or not 0 < task[key] <= 3600:
                raise ValueError("Invalid task deadline: " + str(path))
        for required in ("workspace", "verifier/verify.py", "instruction.md", "solution"):
            if not (path.parent / required).exists():
                raise ValueError("Missing task component: " + required)
        task["path"] = path.parent
        tasks.append(task)
    unknown = set(selected or []) - {task["id"] for task in tasks}
    if unknown or not tasks:
        raise ValueError("Unknown or empty task selection: " + repr(sorted(unknown)))
    return tasks


def snapshot(directory):
    """Never follow candidate symlinks or read special files on the host."""
    result = {}
    for base, directories, files in os.walk(directory, followlinks=False):
        directories[:] = sorted(name for name in directories if name not in (".git", "__pycache__"))
        for name in list(directories):
            path = Path(base) / name
            if path.is_symlink():
                raise ValueError("Candidate symlinks are unsupported: " + str(path.relative_to(directory)))
        for name in sorted(files):
            path = Path(base) / name
            if path.is_symlink() or not path.is_file():
                raise ValueError("Candidate special files are unsupported: " + str(path.relative_to(directory)))
            if path.stat().st_size > 2 * 1024 * 1024:
                raise ValueError("Candidate file exceeds pilot limit: " + str(path.relative_to(directory)))
            if len(result) >= 500:
                raise ValueError("Candidate exceeds pilot file limit")
            result[str(path.relative_to(directory))] = path.read_bytes()
    return result


def fingerprints(files):
    return {name: hashlib.sha256(data).hexdigest() for name, data in files.items()}


def candidate_diff(before, after):
    chunks = []
    for name in sorted(before.keys() | after.keys()):
        old, new = before.get(name, b""), after.get(name, b"")
        if old != new:
            chunks.extend(difflib.unified_diff(old.decode(errors="replace").splitlines(True),
                                             new.decode(errors="replace").splitlines(True),
                                             fromfile="a/" + name, tofile="b/" + name))
    return "".join(chunks)


def bind(directory, target, readonly=False):
    path = str(directory.resolve())
    if "," in path:
        raise ValueError("Docker mount paths must not contain commas")
    return ["--mount", "type=bind,source=" + path + ",target=" + target + (",readonly" if readonly else "")]


def sandbox_args(name, network="none"):
    if os.getuid() == 0:
        raise ValueError("Run the lab as a non-root user with Docker access")
    return ["docker", "run", "--rm", "--name", name, "--network", network,
            "--read-only", "--cap-drop", "ALL", "--security-opt", "no-new-privileges",
            "--pids-limit", "256", "--cpus", "2", "--memory", "2g", "--memory-swap", "2g",
            "--tmpfs", "/tmp:rw,nosuid,nodev,size=512m", "--ulimit", "fsize=67108864:67108864",
            "--user", str(os.getuid()) + ":" + str(os.getgid())]


def secret_patterns(secrets):
    patterns = set()
    for secret in secrets:
        if secret:
            patterns.add(secret.encode())
            for ascii_only in (True, False):
                patterns.add(json.dumps(secret, ensure_ascii=ascii_only)[1:-1].encode())
    return sorted(patterns, key=len, reverse=True)


class StreamRedactor:
    """Retain enough tail bytes to redact secrets split across capture chunks."""
    def __init__(self, secrets):
        self.patterns = secret_patterns(secrets)
        self.tail = max((len(pattern) for pattern in self.patterns), default=1) - 1
        self.pending = b""

    def feed(self, data, final=False):
        self.pending += data
        boundary = len(self.pending) if final else max(0, len(self.pending) - self.tail)
        position, output = 0, []
        while position < boundary:
            matches = [(self.pending.find(pattern, position), pattern) for pattern in self.patterns]
            matches = [(start, pattern) for start, pattern in matches if position <= start < boundary]
            if not matches:
                output.append(self.pending[position:boundary])
                position = boundary
                break
            start, pattern = min(matches, key=lambda match: (match[0], -len(match[1])))
            output.extend((self.pending[position:start], b"[REDACTED]"))
            position = start + len(pattern)
        self.pending = self.pending[position:]
        return b"".join(output)


def redact(text):
    redactor = StreamRedactor([os.environ.get(key, "") for key in ("OPENAI_API_KEY", "ANTHROPIC_API_KEY")])
    return redactor.feed(text.encode(), final=True).decode(errors="replace")


def resolve_credential(args, arm, minimum_validity_seconds):
    provider = "openai" if ARMS[arm]["adapter"] == "neoism" else "anthropic"
    if getattr(args, "use_neoism_auth", False):
        document = saved_auth.load_document(getattr(args, "auth_path", None))
        selector = getattr(args, provider + "_connection", None)
        credential = saved_auth.select_credential(document, provider, selector)
        return saved_auth.projection(provider, credential, minimum_validity_seconds)
    if any(getattr(args, key, None) for key in ("auth_path", "openai_connection", "anthropic_connection")):
        raise ValueError("Auth path/connection selectors require --use-neoism-auth")
    key = os.environ.get(ARMS[arm]["credential"])
    if not key:
        raise ValueError("Missing " + ARMS[arm]["credential"] + "; use --use-neoism-auth to reuse saved Neoism credentials")
    return {"type": "api", "key": key}


def cleanup_container(name):
    """Fail closed if Docker cannot confirm this specific container was removed."""
    for _ in range(2):
        try:
            subprocess.run(["docker", "rm", "--force", name], stdout=subprocess.DEVNULL,
                           stderr=subprocess.DEVNULL, timeout=10, check=False)
            check = subprocess.run(["docker", "ps", "--all", "--quiet", "--filter", "name=^/" + name + "$"],
                                   capture_output=True, text=True, timeout=10, check=False)
            if check.returncode == 0 and not check.stdout.strip():
                return
        except subprocess.SubprocessError:
            continue
    raise RuntimeError("Cannot confirm cleanup of " + name + "; stop paid runs and inspect Docker")


def container(image, output, label, worker_args, mounts=(), network="none", secrets=(),
              prompt=None, timeout=90, writable_workspace=False, trusted_verifier=False):
    name = "neoism-eval-" + uuid.uuid4().hex[:16]
    args = sandbox_args(name, network)
    if trusted_verifier:
        args[args.index("--user") + 1] = "0:0"
        args.extend(["--cap-add", "SETUID", "--cap-add", "SETGID"])
    if prompt is not None:
        args.append("-i")
    if writable_workspace:
        args.extend(["--tmpfs", "/workspace:rw,nosuid,nodev,size=64m,uid=" +
                     str(os.getuid()) + ",gid=" + str(os.getgid())])
    for source, target, readonly in mounts:
        args.extend(bind(source, target, readonly))
    args.extend([image] + worker_args)
    stdout_path, stderr_path = output / (label + ".jsonl"), output / (label + ".stderr.log")
    started = time.monotonic()
    timed_out, log_limited, returncode = False, False, None
    limit_reached = threading.Event()

    def capture(stream, path):
        remaining = output_remaining = 16 * 1024 * 1024
        redactor = StreamRedactor(secrets)
        with path.open("wb") as destination:
            while True:
                data = stream.read(65536)
                if not data:
                    if remaining and output_remaining:
                        destination.write(redactor.feed(b"", final=True)[:output_remaining])
                    break
                accepted = data[:remaining]
                remaining -= len(accepted)
                if output_remaining:
                    captured = redactor.feed(accepted)
                    destination.write(captured[:output_remaining])
                    output_remaining -= min(output_remaining, len(captured))
                if not remaining or not output_remaining:
                    limit_reached.set()

    process = subprocess.Popen(args, stdin=subprocess.PIPE if prompt is not None else subprocess.DEVNULL,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    readers = [threading.Thread(target=capture, args=(stream, path), daemon=True)
               for stream, path in ((process.stdout, stdout_path), (process.stderr, stderr_path))]
    for reader in readers:
        reader.start()
    cleanup_error = None
    try:
        if prompt is not None:
            assert process.stdin is not None
            try:
                process.stdin.write(prompt.encode())
                process.stdin.close()
            except BrokenPipeError:
                pass
        while process.poll() is None:
            if limit_reached.is_set():
                log_limited = True
                break
            if time.monotonic() - started >= timeout:
                timed_out = True
                break
            time.sleep(0.05)
        returncode = process.poll()
    finally:
        try:
            cleanup_container(name)
        except RuntimeError as error:
            cleanup_error = error
        finally:
            if process.poll() is None:
                process.kill()
            process.wait(timeout=10)
            for reader in readers:
                reader.join(timeout=10)
            for stream in (process.stdout, process.stderr, process.stdin):
                if stream is not None:
                    stream.close()
    log_limited = log_limited or limit_reached.is_set()
    if cleanup_error:
        raise cleanup_error
    return {"returncode": returncode, "timed_out": timed_out, "log_limited": log_limited,
            "duration_seconds": round(time.monotonic() - started, 3)}


def restore_candidate(files, destination):
    """Validate the entire bounded export before creating any host-side paths."""
    if not isinstance(files, dict) or len(files) > 500:
        raise ValueError("Invalid workspace export")
    decoded, total = {}, 0
    for name, encoded in files.items():
        if not isinstance(name, str) or not isinstance(encoded, str) or len(encoded) > 3 * 1024 * 1024:
            raise ValueError("Invalid exported file")
        path = PurePosixPath(name)
        if path.is_absolute() or not path.parts or any(part in ("..", ".git") for part in path.parts) or str(path) != name:
            raise ValueError("Unsafe exported path: " + name)
        data = base64.b64decode(encoded, validate=True)
        total += len(data)
        if len(data) > 2 * 1024 * 1024 or total > 8 * 1024 * 1024:
            raise ValueError("Workspace export exceeds pilot limits")
        decoded[name] = data
    for name in decoded:
        if any(str(parent) in decoded for parent in PurePosixPath(name).parents):
            raise ValueError("Conflicting exported paths")
    destination.mkdir()
    for name, data in decoded.items():
        path = destination / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)


def read_events(path):
    events = []
    for line in path.read_text(errors="replace").splitlines():
        try:
            value = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(value, dict):
            events.append(value)
    return events


def preflight(image, output):
    result = container(image, output, "preflight", ["python3", "/opt/eval/worker.py", "preflight"])
    events = read_events(output / "preflight.jsonl")
    event = next((event for event in events if event.get("type") == "preflight"), None)
    if result["returncode"] != 0 or not event or not event.get("native_server_ready"):
        raise RuntimeError("Offline agent preflight failed; see " + str(output / "preflight.stderr.log"))
    for filename, key in (("worker.py", "worker_sha256"), ("verify.py", "verifier_sha256")):
        if event.get(key) != hashlib.sha256((ROOT / filename).read_bytes()).hexdigest():
            raise RuntimeError("Image adapter differs from current source; rebuild the lab image")
    for filename in ("candidate_client.py", "candidate_worker.py"):
        if event.get("candidate_hashes", {}).get(filename) != hashlib.sha256((ROOT / filename).read_bytes()).hexdigest():
            raise RuntimeError("Image candidate transport differs from current source; rebuild the lab image")
    return event


def new_output(prefix):
    runs = ROOT / "runs"
    runs.mkdir(exist_ok=True)
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    output = runs / (prefix + "-" + stamp + "-" + uuid.uuid4().hex[:8])
    output.mkdir(mode=0o700)
    return output


def provenance(image, tasks):
    return {"image_id": image, "created_at": datetime.now(timezone.utc).isoformat(),
            "runner_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
            "worker_sha256": hashlib.sha256((ROOT / "worker.py").read_bytes()).hexdigest(),
            "verifier_sha256": hashlib.sha256((ROOT / "verify.py").read_bytes()).hexdigest(),
            "candidate_hashes": {name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest()
                                 for name in ("candidate_client.py", "candidate_worker.py")},
            "tasks": {task["id"]: fingerprints(snapshot(task["path"])) for task in tasks},
            "resource_limits": {"cpus": 2, "memory": "2g", "pids": 256},
            "limitations": ["Private synthetic Python task suite, not an external benchmark or universal ranking",
                            "Different models compare complete setups, not harness alone",
                            "Native reported cost is not a billing invoice",
                            "Neoism observed-cost cutoff can overshoot; no hard dollar guarantee",
                            "Paid agents have outbound network access; verifier has none"]}


def trial(image, output, task, arm, repeat, effort="high", budget=None, credential=None):
    folder = output / (task["id"] + "--" + arm + "--" + str(repeat))
    folder.mkdir(mode=0o700)
    workspace = folder / "workspace"
    shutil.copytree(task["path"] / "workspace", workspace)
    before = snapshot(workspace)
    result = {"task": task["id"], "arm": arm, "repeat": repeat, "status": "completed",
              "reported_cost_usd": 0.0 if arm in ("baseline", "oracle") else None,
               "model": ARMS.get(arm, {}).get("model"), "effort": effort,
               "suite": task.get("suite", "smoke"), "category": task.get("category", "smoke"),
               "duration_seconds": 0.0, "passed": False}
    events = []
    if arm == "oracle":
        shutil.copytree(task["path"] / "solution", workspace, dirs_exist_ok=True)
    elif arm != "baseline":
        if budget is None:
            raise ValueError("Paid trials require an explicit budget")
        spec = ARMS[arm]
        if credential is None:
            raise ValueError("Contestant credential must be selected before starting a trial")
        result["auth_type"] = credential["type"]
        result["billing_basis"] = "subscription" if credential["type"] == "oauth" else "api_estimate"
        prompt = (task["path"] / "instruction.md").read_text() + (
            "\n\nWork autonomously in /workspace. Do not ask follow-up questions. "
            "Run available tests and leave your implementation in the workspace.\n")
        execution = container(image, folder, "agent", ["python3", "/opt/eval/worker.py", spec["adapter"],
                              "--model", spec["model"], "--effort", effort, "--budget", str(budget),
                              "--timeout", str(task["timeout_seconds"])],
                               mounts=[(workspace, "/seed", True)], network="bridge",
                               secrets=saved_auth.credential_secrets(credential),
                               prompt=json.dumps({"prompt": prompt, "credential": credential}), writable_workspace=True,
                               timeout=task["timeout_seconds"] + 75)
        events = read_events(folder / "agent.jsonl")
        summaries = [event for event in events if event.get("type") == "lab_result"]
        if summaries:
            result.update({key: value for key, value in summaries[-1].items() if key != "type"})
        else:
            result["status"] = "missing_agent_result"
        result["duration_seconds"] = execution["duration_seconds"]
        if execution["timed_out"]:
            result["status"] = "timeout"
        elif execution.get("log_limited"):
            result["status"] = "log_limit"
        elif execution["returncode"] != 0 and result["status"] == "completed":
            result["status"] = "adapter_error"
        result["agent_execution"] = execution
    try:
        if arm in ARMS:
            exports = [event for event in events if event.get("type") == "lab_workspace"]
            if len(exports) != 1:
                raise ValueError("Missing or ambiguous bounded workspace export")
            workspace = folder / "candidate"
            restore_candidate(exports[0]["files"], workspace)
        after = snapshot(workspace)
        write_json(folder / "workspace_hashes.json", {"before": fingerprints(before), "after": fingerprints(after)})
        (folder / "candidate.diff").write_text(candidate_diff(before, after))
        verification = container(image, folder, "verifier", ["python3", "/opt/eval/verify.py"],
                                 mounts=[(workspace, "/workspace", True)], trusted_verifier=True,
                                 prompt=(task["path"] / "verifier" / "verify.py").read_text(),
                                 timeout=task["verifier_timeout_seconds"])
        outcomes = [event for event in read_events(folder / "verifier.jsonl")
                    if event.get("type") == "verification_result"]
        outcome = outcomes[0] if len(outcomes) == 1 else None
        valid = (verification["returncode"] in (0, 1) and not verification["timed_out"]
                 and not verification.get("log_limited") and outcome is not None
                 and outcome.get("tests_run", 0) > 0 and outcome.get("skipped") == 0)
        result["verification"] = dict(verification, outcome=outcome, valid=valid)
        result["acceptance_passed"] = bool(valid and verification["returncode"] == 0
                                           and outcome is not None and outcome["failures"] == 0
                                           and outcome["errors"] == 0)
        if not valid:
            result["status"] = "verifier_error" if result["status"] == "completed" else result["status"]
        result["passed"] = result["acceptance_passed"] and result["status"] == "completed"
    except (ValueError, OSError) as error:
        result.update(candidate_error=str(error), acceptance_passed=False)
        if result["status"] == "completed":
            result["status"] = "invalid_candidate"
    if arm in ARMS:
        cost = result.get("reported_cost_usd")
        cost_valid = not isinstance(cost, bool) and isinstance(cost, (int, float)) and math.isfinite(cost) and cost >= 0
        if result.get("billing_basis") == "subscription":
            result["estimated_usage_cost_usd"] = cost if cost_valid else None
            result["reported_cost_usd"] = None
            if not result.get("accounting_complete") or not result.get("usage_metered"):
                result["status"] = "unmetered_usage" if result["status"] == "completed" else result["status"]
                result["passed"] = False
            elif cost_valid and budget is not None and cost > budget:
                result.update(status="budget_exceeded", passed=False)
        else:
            if result.get("accounting_complete") is False:
                result["partial_reported_cost_usd"] = cost
                result["reported_cost_usd"] = cost = None
                cost_valid = False
            if not cost_valid or cost is None or cost <= 0:
                result["reported_cost_usd"] = None
                result["status"] = "unmetered_usage" if result["status"] == "completed" else result["status"]
                result["passed"] = False
            elif budget is not None and cost > budget:
                result.update(status="budget_exceeded", passed=False)
    write_json(folder / "result.json", result)
    print(task["id"] + " / " + arm + ": " + ("PASS" if result["passed"] else "FAIL") +
          " (" + result["status"] + ")", flush=True)
    return result


def report(output):
    results = [json.loads(path.read_text()) for path in sorted(output.glob("*/result.json"))]
    groups = defaultdict(list)
    for result in results:
        groups[result["arm"]].append(result)
    manifest = json.loads((output / "manifest.json").read_text()) if (output / "manifest.json").exists() else {}
    suite = manifest.get("suite", "smoke")
    description = ("Private synthetic multi-file Python repository suite. Not a public benchmark or a universal ranking."
                   if suite in ("repository", "all") else "Public smoke tasks only. These results do not establish a general winner.")
    lines = ["# Coding-agent pilot results", "", description, ""]
    if manifest.get("plan"):
        completed = {(item["task"], item["arm"], item["repeat"]) for item in results}
        planned = {(item["task"], item["arm"], item["repeat"]) for item in manifest["plan"]}
        lines.extend([f"Plan completion: **{len(completed & planned)}/{len(planned)} trials**.", ""])
        if completed & planned != planned:
            lines.extend(["**INCOMPLETE EXPERIMENT: do not interpret the partial score as a completed comparison.**", ""])
    if (output / "budget.json").exists():
        budget = json.loads((output / "budget.json").read_text())
        lines.extend(["Known API spend including prior runs: $" + format(budget["known_api_spend_usd"], ".4f") +
                      " / $" + format(budget["maximum_api_budget_usd"], ".2f") + " authorized (runtime soft limits).", ""])
        if not budget.get("api_accounting_complete"):
            lines.extend(["**API cost accounting is incomplete; known spend is a lower bound.**", ""])
    lines.extend(["| Setup | Passed | Success rate | Reported cost | Cost / pass | Median successful time |",
                  "| --- | ---: | ---: | ---: | ---: | ---: |"])
    for arm, items in sorted(groups.items()):
        passed = sum(item["passed"] for item in items)
        costs = [item.get("reported_cost_usd") for item in items]
        total = sum(costs) if all(cost is not None for cost in costs) else None
        durations = [item["duration_seconds"] for item in items if item["passed"]]
        subscription = any(item.get("billing_basis") == "subscription" for item in items)
        cost_text = "subscription (not USD-priced)" if subscription else ("$" + format(total, ".4f") if total is not None else "unknown")
        per_pass = "$" + format(total / passed, ".4f") if not subscription and total is not None and passed else "n/a"
        timing = format(statistics.median(durations), ".2f") + "s" if durations else "n/a"
        lines.append(f"| {arm} | {passed}/{len(items)} | {100 * passed / len(items):.1f}% | {cost_text} | {per_pass} | {timing} |")
    lines.extend(["", "Failures and timeouts count in the denominator. Unknown costs are never treated as zero.",
                  "Reported API costs include failures when available, but are not verified invoices.",
                  "OAuth subscription usage is scored by correctness/time and token usage, not ranked as free or comparable USD spend.", ""])
    if len(groups) == 2 and set(groups) != {"baseline", "oracle"}:
        left, right = sorted(groups)
        indexed = {(item["task"], item["repeat"], item["arm"]): item for item in results}
        wins = [0, 0, 0]
        for task, repeat in sorted({(item["task"], item["repeat"]) for item in results}):
            a, b = indexed.get((task, repeat, left)), indexed.get((task, repeat, right))
            if a is not None and b is not None:
                wins[2 if a["passed"] == b["passed"] else (0 if a["passed"] else 1)] += 1
        lines.append(f"Paired outcomes: {left} only {wins[0]}, {right} only {wins[1]}, ties {wins[2]}.")
    contestant_groups = sorted(arm for arm in groups if arm in ARMS)
    if contestant_groups:
        task_ids = sorted({item["task"] for item in results if item["arm"] in ARMS})
        lines.extend(["", "## Per-task reliability", "",
                      "| Task | " + " | ".join(contestant_groups) + " |",
                      "| --- | " + " | ".join("---:" for _ in contestant_groups) + " |"])
        for task in task_ids:
            cells = []
            for arm in contestant_groups:
                attempts = [item for item in groups[arm] if item["task"] == task]
                cells.append(str(sum(item["passed"] for item in attempts)) + "/" + str(len(attempts)))
            lines.append("| " + task + " | " + " | ".join(cells) + " |")
        lines.extend(["", "Each cell is successful attempts / completed attempts. Repeated attempts on one task are not independent tasks."])
    (output / "report.md").write_text("\n".join(lines) + "\n")
    write_json(output / "results.json", results)
    return results


def build(args):
    binary = Path(args.neoism_agent or shutil.which("neoism-agent") or "").resolve()
    if not binary.is_file() or not os.access(binary, os.X_OK):
        raise ValueError("Provide an executable --neoism-agent path")
    # Only the chosen binary and two public harness files enter the build context.
    with tempfile.TemporaryDirectory(prefix="neoism-eval-build-") as temporary:
        context = Path(temporary)
        shutil.copy2(binary, context / "neoism-agent")
        for name in ("Dockerfile", "worker.py", "verify.py", "candidate_client.py", "candidate_worker.py"):
            shutil.copy2(ROOT / name, context / name)
        subprocess.run(["docker", "build", "--build-arg", "CLAUDE_VERSION=" + args.claude_version,
                        "--tag", args.image, str(context)], check=True)
    output = new_output("build")
    write_json(output / "build.json", {"image_id": image_id(args.image),
               "native_binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
               "native_version": command([str(binary), "--version"]), "claude_version": args.claude_version})
    print("Built " + args.image + "; provenance: " + str(output))


def doctor(args):
    print("Offline readiness only; credentials are never printed and no inference is requested.")
    auth_status = {}
    if getattr(args, "use_neoism_auth", False):
        for arm in ("neoism-sol", "claude-sonnet"):
            try:
                credential = resolve_credential(args, arm, 345)
                auth_status[arm] = {"available_for_trial": True, "type": credential["type"],
                                    "source": "neoism", "authenticated_online": False}
            except (ValueError, OSError) as error:
                auth_status[arm] = {"available_for_trial": False, "reason": str(error), "source": "neoism"}
        print(json.dumps(auth_status, indent=2))
    else:
        for key in ("OPENAI_API_KEY", "ANTHROPIC_API_KEY"):
            print(key + ": " + ("set (not authenticated)" if os.environ.get(key) else "not set"))
    image = image_id(args.image)
    output = new_output("doctor")
    result = preflight(image, output)
    write_json(output / "preflight.json", dict(result, image_id=image, saved_auth_status=auth_status))
    print(json.dumps(result, indent=2))
    print("Offline readiness evidence: " + str(output))


def calibrated_negative(result):
    verification = result.get("verification", {})
    outcome = verification.get("outcome") or {}
    return (result["status"] == "completed" and not result["passed"]
            and verification.get("valid") and verification.get("returncode") == 1
            and outcome.get("tests_run", 0) > 0 and outcome.get("failures", 0) > 0
            and outcome.get("errors") == 0 and outcome.get("skipped") == 0)


def self_test(args):
    tasks, image = load_tasks(args.task, getattr(args, "suite", "smoke")), image_id(args.image)
    output = new_output("self-test")
    write_json(output / "manifest.json", dict(provenance(image, tasks), mode="offline-self-test",
                                             suite=getattr(args, "suite", "smoke")))
    preflight(image, output)
    results = [trial(image, output, task, arm, 1) for task in tasks for arm in ("baseline", "oracle")]
    report(output)
    if any(not (item["passed"] if item["arm"] == "oracle" else calibrated_negative(item)) for item in results):
        raise RuntimeError("Verifier calibration failed; inspect " + str(output))
    print("Calibration passed: every broken baseline fails; every oracle passes. No model calls.")
    print("Report: " + str(output / "report.md"))


def prior_api_spend(directories):
    pending = [Path(directory).resolve() for directory in directories or []]
    if len(set(pending)) != len(pending):
        raise ValueError("Prior runs must be unique")
    total, seen = 0.0, set()
    while pending:
        path = pending.pop()
        if path in seen:
            continue
        seen.add(path)
        items = json.loads((path / "results.json").read_text())
        manifest = json.loads((path / "manifest.json").read_text()) if (path / "manifest.json").exists() else {}
        pending.extend(Path(parent).resolve() for parent in manifest.get("prior_runs", []))
        completed = {(item.get("task"), item.get("arm"), item.get("repeat")) for item in items}
        ledger = json.loads((path / "budget.json").read_text()) if (path / "budget.json").exists() else None
        if ledger is not None:
            if ledger.get("api_accounting_complete") is not True or ledger.get("in_flight_api_trial"):
                raise ValueError("A prior run has interrupted or incomplete API accounting; reconcile it before more spend")
            started = {(item["task"], item["arm"], item["repeat"]) for item in ledger.get("started_api_trials", [])}
            if not started <= completed:
                raise ValueError("A started prior API trial is missing its result")
        elif manifest.get("plan"):
            planned = {(item["task"], item["arm"], item["repeat"]) for item in manifest["plan"]}
            if not planned <= completed:
                raise ValueError("A prior run has missing trials without a safe accounting ledger")
        own_cost = 0.0
        for item in items:
            if item.get("billing_basis") == "subscription" or item.get("arm") in ("baseline", "oracle"):
                continue
            cost = item.get("reported_cost_usd")
            if isinstance(cost, bool) or not isinstance(cost, (int, float)) or not math.isfinite(cost) or cost < 0:
                raise ValueError("A prior API trial has unknown cost; reconcile it before authorizing more spend")
            own_cost += cost
        if ledger is not None and not math.isclose(own_cost + ledger.get("prior_api_spend_usd", 0),
                                                   ledger["known_api_spend_usd"], rel_tol=1e-9, abs_tol=1e-9):
            raise ValueError("Prior API results and budget ledger disagree")
        total += own_cost
    return total


def validate_paid(args, tasks):
    if not args.allow_paid:
        raise ValueError("Paid execution is disabled. An explicit --allow-paid is required.")
    if args.budget_usd is None or args.trial_budget_usd is None:
        raise ValueError("Both --budget-usd and --trial-budget-usd are required")
    if len(set(args.arms)) != len(args.arms):
        raise ValueError("Contestant arms must be unique")
    spent = prior_api_spend(getattr(args, "prior_run", None))
    if spent >= args.budget_usd:
        raise ValueError("Prior API spend already exhausts the experiment budget")
    if any(ARMS[arm]["adapter"] == "neoism" for arm in args.arms) and not args.accept_soft_budget:
        raise ValueError("Neoism has no hard dollar cap. --accept-soft-budget explicitly acknowledges possible overshoot; use a provider-side limit too.")
    for arm in args.arms:
        resolve_credential(args, arm, max(task["timeout_seconds"] for task in tasks) + 165)


def run(args):
    tasks = load_tasks(args.task, getattr(args, "suite", "smoke"))
    validate_paid(args, tasks)
    spent = initial_spend = prior_api_spend(getattr(args, "prior_run", None))
    image = image_id(args.image)
    output = new_output("comparison")
    plans = [(task, arm, repeat) for task in tasks for repeat in range(1, args.repeat + 1) for arm in args.arms]
    random.Random(args.seed).shuffle(plans)
    write_json(output / "manifest.json", dict(provenance(image, tasks), mode="paid-pilot",
        arms={arm: ARMS[arm] for arm in args.arms}, effort=args.effort, seed=args.seed,
        budget_usd=args.budget_usd, trial_budget_usd=args.trial_budget_usd,
        prior_api_spend_usd=initial_spend, suite=getattr(args, "suite", "smoke"),
        prior_runs=[str(Path(path).resolve()) for path in getattr(args, "prior_run", []) or []],
        soft_budget_acknowledged=args.accept_soft_budget,
        auth_source="neoism" if getattr(args, "use_neoism_auth", False) else "environment",
        plan=[{"task": task["id"], "arm": arm, "repeat": repeat} for task, arm, repeat in plans]))
    preflight(image, output)
    # Calibrate the exact verifiers/image before any inference can be billed.
    for task in tasks:
        baseline = trial(image, output, task, "baseline", 0)
        oracle = trial(image, output, task, "oracle", 0)
        if not calibrated_negative(baseline) or not oracle["passed"]:
            raise RuntimeError("Pre-run verifier calibration failed: " + str(output))
    halt = None
    ledger = {"maximum_api_budget_usd": args.budget_usd, "prior_api_spend_usd": initial_spend,
              "known_api_spend_usd": spent, "api_accounting_complete": True,
              "started_api_trials": [], "in_flight_api_trial": None}
    write_json(output / "budget.json", ledger)
    try:
        for task, arm, repeat in plans:
            credential = resolve_credential(args, arm, task["timeout_seconds"] + 165)
            subscription = credential["type"] == "oauth"
            remaining = args.budget_usd - spent
            if not subscription and remaining <= 0:
                halt = "API budget exhausted before the full plan completed."
                break
            trial_limit = args.trial_budget_usd if subscription else min(args.trial_budget_usd, remaining)
            if not subscription:
                identity = {"task": task["id"], "arm": arm, "repeat": repeat}
                ledger["started_api_trials"].append(identity)
                ledger["in_flight_api_trial"] = identity
                ledger["api_accounting_complete"] = False
                write_json(output / "budget.json", ledger)
            result = trial(image, output, task, arm, repeat, args.effort, trial_limit, credential)
            if not subscription:
                cost = result.get("reported_cost_usd")
                if cost is None:
                    ledger["api_accounting_complete"] = False
                else:
                    spent += cost
                    ledger["api_accounting_complete"] = True
                    ledger["in_flight_api_trial"] = None
            ledger["known_api_spend_usd"] = spent
            ledger["remaining_api_budget_usd"] = max(0, args.budget_usd - spent)
            write_json(output / "budget.json", ledger)
            unknown_api_cost = not subscription and result.get("reported_cost_usd") is None
            subscription_adapter_failure = subscription and result["status"] in ("adapter_error", "missing_agent_result")
            if (unknown_api_cost or subscription_adapter_failure or result.get("model_mismatch") or
                    result["status"] in ("budget_exceeded", "model_mismatch") or spent > args.budget_usd):
                halt = "Stopped after unknown API cost, adapter failure, model mismatch, or budget overrun."
                break
    finally:
        # Keep calibration results separate from contestant scores.
        calibration = output / "calibration"
        calibration.mkdir(exist_ok=True)
        for path in list(output.iterdir()):
            if path.is_dir() and ("--baseline--" in path.name or "--oracle--" in path.name):
                shutil.move(str(path), calibration / path.name)
        report(output)
    print("Report: " + str(output / "report.md"))
    if halt:
        raise RuntimeError(halt)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image", default=DEFAULT_IMAGE)
    commands = parser.add_subparsers(dest="command", required=True)
    build_parser = commands.add_parser("build", help="Build pinned agent image; no model calls")
    build_parser.add_argument("--neoism-agent")
    build_parser.add_argument("--claude-version", default="2.1.292")
    doctor_parser = commands.add_parser("doctor", help="Offline runtime/API readiness; no model calls")
    test_parser = commands.add_parser("self-test", help="Broken baseline + oracle calibration; no model calls")
    test_parser.add_argument("--task", action="append")
    test_parser.add_argument("--suite", choices=["smoke", "repository", "all"], default="smoke")
    run_parser = commands.add_parser("run", help="Explicitly gated paid pilot")
    run_parser.add_argument("--task", action="append")
    run_parser.add_argument("--suite", choices=["smoke", "repository", "all"], default="smoke")
    run_parser.add_argument("--prior-run", type=Path, action="append", help="Count known API spend from these runs against the total budget")
    run_parser.add_argument("--arms", nargs="+", choices=ARMS, default=["neoism-sol", "claude-sonnet"])
    run_parser.add_argument("--repeat", type=positive_integer, default=1)
    run_parser.add_argument("--seed", type=int, default=0)
    run_parser.add_argument("--effort", choices=["low", "medium", "high", "xhigh", "max"], default="high")
    run_parser.add_argument("--allow-paid", action="store_true")
    run_parser.add_argument("--budget-usd", type=finite_positive)
    run_parser.add_argument("--trial-budget-usd", type=finite_positive)
    run_parser.add_argument("--accept-soft-budget", action="store_true")
    for auth_parser in (doctor_parser, run_parser):
        auth_parser.add_argument("--use-neoism-auth", action="store_true", help="Read only the selected credentials from Neoism's saved auth store")
        auth_parser.add_argument("--auth-path", type=Path)
        auth_parser.add_argument("--openai-connection")
        auth_parser.add_argument("--anthropic-connection")
    report_parser = commands.add_parser("report", help="Regenerate an existing run's report")
    report_parser.add_argument("directory", type=Path)
    args = parser.parse_args()
    if (not getattr(args, "use_neoism_auth", False) and
            any(getattr(args, key, None) for key in ("auth_path", "openai_connection", "anthropic_connection"))):
        parser.error("Auth path/connection selectors require --use-neoism-auth")
    if args.command == "report":
        report(args.directory.resolve())
    else:
        {"build": build, "doctor": doctor, "self-test": self_test, "run": run}[args.command](args)


if __name__ == "__main__":
    try:
        main()
    except (ValueError, RuntimeError, OSError, subprocess.SubprocessError) as error:
        print("error: " + redact(str(error)), file=sys.stderr)
        sys.exit(1)
