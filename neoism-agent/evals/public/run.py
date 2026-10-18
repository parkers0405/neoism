#!/usr/bin/env python3
"""Sequential, single-attempt Harbor runner. Offline unless all three execution gates pass.

The budget cap is conservative frozen-catalog reservation, not a provider invoice
promise. The fixed durable ledger is intentionally not configurable. Never delete
it to resume an interrupted dispatched trial.
"""
from __future__ import annotations

import argparse
from contextlib import contextmanager
from datetime import datetime, timedelta, timezone
from decimal import Decimal, InvalidOperation
import fcntl
import hashlib
import json
import math
import os
from pathlib import Path
import subprocess
import tempfile
import time

from readiness import validate_readiness

ROOT = Path(__file__).resolve().parent
RUNS = ROOT / "runs"
LEDGER = RUNS / "public-ledger.json"
LOCK = RUNS / "public-ledger.lock"
TOKEN_KEYS = ("input_uncached", "cache_read", "cache_write", "output_total", "input_total")


def sha256(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def now():
    return datetime.now(timezone.utc).isoformat()


def money(value, positive=False):
    if isinstance(value, bool) or not isinstance(value, (int, float, str, Decimal)):
        raise ValueError("Budget/cost must be a finite number")
    try:
        result = Decimal(str(value))
    except InvalidOperation as exc:
        raise ValueError("Invalid budget/cost") from exc
    if not result.is_finite() or result < 0 or (positive and result == 0):
        raise ValueError("Budget/cost must be finite and nonnegative (budget positive)")
    return result


def safe_path(path, root=None, must_exist=False):
    root = Path(ROOT if root is None else root).resolve()
    path = Path(path)
    path = path if path.is_absolute() else root / path
    if ".." in path.parts or not path.is_relative_to(root):
        raise ValueError("Path escapes benchmark root")
    if any(p.is_symlink() for p in (path, *path.parents)):
        raise ValueError("Symlinks are not allowed: " + str(path))
    if must_exist and not path.is_file():
        raise ValueError("Missing regular file: " + str(path))
    return path


def load(path):
    path = safe_path(path, must_exist=True)
    if path.stat().st_size > 64 * 1024 * 1024:
        raise ValueError("JSON exceeds size limit")
    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError("Duplicate JSON key: " + key)
            result[key] = value
        return result
    return json.loads(path.read_text(), object_pairs_hook=unique)


def sync_directory(path):
    directory = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)


def atomic_json(path, value):
    path = safe_path(path)
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    fd, temporary = tempfile.mkstemp(prefix="." + path.name + ".", dir=path.parent)
    try:
        with os.fdopen(fd, "w") as stream:
            os.fchmod(stream.fileno(), 0o600)
            json.dump(value, stream, indent=2, sort_keys=True, allow_nan=False)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
        # Sync both the replacement and newly created parent directory links.
        for directory in (path.parent, *path.parent.parents):
            sync_directory(directory)
            if directory == ROOT:
                break
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


@contextmanager
def locked_ledger():
    safe_path(LOCK)
    RUNS.mkdir(exist_ok=True, mode=0o700)
    sync_directory(ROOT)
    fd = os.open(LOCK, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    try:
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as exc:
            raise ValueError("Another public runner owns the ledger") from exc
        ledger = load(LEDGER) if LEDGER.exists() else {
            "schema_version": 1, "entries": {}, "experiments": {}, "blocked": None}
        validate_ledger(ledger)
        yield ledger
    finally:
        os.close(fd)


def validate_ledger(ledger):
    if (set(ledger) != {"schema_version", "entries", "experiments", "blocked"}
            or type(ledger["schema_version"]) is not int or ledger["schema_version"] != 1 or not isinstance(ledger["entries"], dict)
            or not isinstance(ledger["experiments"], dict)):
        raise ValueError("Invalid persistent ledger; do not reset it")
    jobs = set()
    for key, entry in ledger["entries"].items():
        if entry.get("state") not in {"in_flight", "blocked", "complete"}:
            raise ValueError("Unknown ledger entry state")
        if entry.get("key") != key or entry.get("source_job") in jobs:
            raise ValueError("Ledger has duplicate source jobs or identities")
        jobs.add(entry["source_job"])
        cap = money(entry["maximum_api_cost_usd"], positive=True)
        spent = money(entry["charged_cost_usd"])
        if spent > cap or (entry["state"] != "complete" and spent != cap):
            raise ValueError("Ledger reservation is inconsistent")


def checked_plan():
    # Preparation owns source/config/artifact provenance. Never import this module
    # from prepare.py: both preparation and readiness must remain inference-free.
    from lab import verify_plan
    verify_plan()
    plan = load(ROOT / "plan.json")
    protocol = load(ROOT / "protocol.json")
    if (plan.get("schema_version") != 2 or plan.get("model") != protocol["model"]
            or type(plan.get("attempts")) is not int or plan["attempts"] != 1
            or plan.get("tasks") != 51 or plan.get("harnesses") != 3
            or len(plan.get("trials", [])) != 153
            or plan.get("protocol_sha256") != sha256(ROOT / "protocol.json")
            or plan.get("dataset_lock_sha256") != sha256(ROOT / "hard51.lock.json")):
        raise ValueError("Plan does not match the pinned 153-trial protocol")
    pairs, slots, jobs = set(), set(), set()
    tasks = {t["id"] for t in load(ROOT / "hard51.lock.json")["tasks"]}
    for slot in plan["trials"]:
        pair = (slot["task"], slot["harness"])
        if (type(slot.get("attempt")) is not int or slot["attempt"] != 1 or slot["task"] not in tasks
                or slot["harness"] not in protocol["harnesses"]
                or pair in pairs or slot["slot"] in slots
                or not slot["slot"] or Path(slot["slot"]).name != slot["slot"]):
            raise ValueError("Duplicate or incompatible task/harness/slot")
        pairs.add(pair)
        slots.add(slot["slot"])
        source = checked_config(slot, protocol, regrade=False)
        replay = checked_config(slot, protocol, regrade=True)
        for config in (source, replay):
            job = str(job_path(config))
            if job in jobs:
                raise ValueError("Source/regrade job directories must be unique")
            jobs.add(job)
        replay_kwargs = replay["agents"][0].get("kwargs", {})
        if replay_kwargs.get("source_job") != str(job_path(source)):
            raise ValueError("Pristine regrade is not bound to the original source job")
    return plan, protocol, sha256(ROOT / "plan.json")


def job_path(config):
    name = config["job_name"]
    if not isinstance(name, str) or Path(name).name != name or name in {".", "..", ""}:
        raise ValueError("Invalid Harbor job name")
    return safe_path(Path(config["jobs_dir"]) / name, root=RUNS)


def frozen_agent_kwargs(slot, protocol):
    assets = load(ROOT / "assets.lock.json")
    catalog = load(ROOT / "model-catalog.lock.json")
    artifact = assets["agents"][slot["harness"]]
    expected = {"binary_path": artifact["binary_path"], "binary_sha256": artifact["binary_sha256"],
                "version": protocol["harnesses"][slot["harness"]]["version"],
                "catalog_path": str(ROOT / ".cache/models.json"), "catalog_sha256": catalog["sha256"]}
    if "runtime_bundle" in artifact:
        expected.update(runtime_bundle=artifact["runtime_bundle"], runtime_bundle_sha256=assets["runtime_bundle"]["sha256"])
    return expected


def checked_config(slot, protocol, regrade=False):
    config = load(slot["regrade_config" if regrade else "job_config"])
    allowed = {"job_name", "jobs_dir", "n_attempts", "n_concurrent_trials", "retry", "environment", "agents", "tasks"}
    if (set(config) != allowed or type(config.get("n_attempts")) is not int or config["n_attempts"] != 1
            or type(config.get("n_concurrent_trials")) is not int or config["n_concurrent_trials"] != 1
            or config.get("retry") != {"max_retries": 0} or type(config["retry"]["max_retries"]) is not int
            or config.get("environment") != {"type": "docker"}
            or len(config.get("agents", [])) != 1 or len(config.get("tasks", [])) != 1
            or config["tasks"] != [{"path": str(ROOT / ".cache/hard51" / slot["task"])}]):
        raise ValueError("Unexpected job overrides, retries, or multi-task source job")
    agent = config["agents"][0]
    if regrade:
        if (set(agent) != {"import_path", "model_name", "kwargs"}
                or agent["import_path"] != "neoism_harbor.replay:PatchReplayAgent"
                or agent["model_name"] != "replay" or set(agent["kwargs"]) != {"source_job"}):
            raise ValueError("Unexpected pristine replay configuration")
    else:
        if (set(agent) != {"import_path", "model_name", "extra_allowed_hosts", "kwargs"}
                or agent["import_path"] != protocol["harnesses"][slot["harness"]]["class"]
                or agent["model_name"] != "anthropic/" + protocol["model"]["id"]
                or agent["extra_allowed_hosts"] != ["api.anthropic.com"]
                or agent.get("kwargs") != frozen_agent_kwargs(slot, protocol)):
            raise ValueError("Unexpected agent/model/auth/network override")
    job_path(config)
    return config


def approval_template(plan, protocol, plan_sha):
    return {"approved": False, "budget_control_verified": False,
            "maximum_api_cost_usd": None, "maximum_trial_api_cost_usd": None,
            "expires_at": None, "plan_sha256": plan_sha,
            "dataset_commit": protocol["dataset"]["commit"], "model": plan["model"]["id"],
            "attempts": 1,
            "budget_semantics": "Conservative frozen-catalog reservation, not provider invoice guarantee"}


def validate_approval(path, plan, protocol, plan_sha):
    document = load(path)
    allowed = set(approval_template(plan, protocol, plan_sha))
    if set(document) - allowed:
        raise ValueError("Unexpected approval overrides")
    if (document.get("approved") is not True or document.get("budget_control_verified") is not True
            or document.get("plan_sha256") != plan_sha
            or document.get("dataset_commit") != protocol["dataset"]["commit"]
            or document.get("model") != plan["model"]["id"] or type(document.get("attempts")) is not int
            or document["attempts"] != 1):
        raise ValueError("Approval must explicitly match this dataset/model/attempt/plan")
    if any(type(document.get(key)) not in (int, float) for key in ("maximum_api_cost_usd", "maximum_trial_api_cost_usd")):
        raise ValueError("Approval budgets must be JSON numbers, not strings or booleans")
    total = money(document.get("maximum_api_cost_usd"), positive=True)
    trial = money(document.get("maximum_trial_api_cost_usd"), positive=True)
    if trial > total:
        raise ValueError("Trial reservation exceeds global maximum")
    try:
        expiry = datetime.fromisoformat(document["expires_at"].replace("Z", "+00:00"))
    except (KeyError, TypeError, ValueError, AttributeError) as exc:
        raise ValueError("Approval requires a UTC ISO expiry") from exc
    if expiry.tzinfo is None or expiry.utcoffset() != timedelta(0) or expiry <= datetime.now(timezone.utc):
        raise ValueError("Approval expired or expiry is not UTC")
    return document, total, trial


def identity(slot, protocol):
    # Cross-experiment identity deliberately excludes seed, plan hash and slot order.
    return ":".join((protocol["dataset"]["commit"], protocol["model"]["id"], slot["task"], slot["harness"], "1"))


def grade(path):
    """A missing/exceptional verifier is infrastructure error, never a failed solve."""
    if not path.is_file():
        return {"status": "infra_error", "reward": None}
    result = load(path)
    verifier = result.get("verifier_result")
    rewards = verifier.get("rewards") if isinstance(verifier, dict) else None
    reward = rewards.get("reward") if isinstance(rewards, dict) else None
    if (not result.get("finished_at") or result.get("exception_info") or not isinstance(reward, (int, float))
            or isinstance(reward, bool) or reward not in (0, 1)):
        return {"status": "infra_error", "reward": None}
    return {"status": "passed" if reward == 1 else "failed", "reward": reward}


def one_trial_result(job):
    paths = [p for p in job.glob("*/result.json") if p.is_file()]
    if len(paths) != 1:
        raise ValueError("Expected exactly one Harbor trial result")
    return paths[0]


def collect_source(job, model, cap):
    summaries = list(job.glob("*/agent/eval-result.json"))
    if len(summaries) != 1:
        raise ValueError("Missing/ambiguous downloaded agent eval-result.json; spend unknown")
    artifact = safe_path(summaries[0], must_exist=True)
    result = load(artifact)
    if result.get("accounting_complete") is not True or type(result.get("inference_started")) is not bool:
        raise ValueError("Unknown inference accounting")
    models = result.get("observed_models")
    if not isinstance(models, list) or (set(models) != {model} if result["inference_started"] else bool(models)):
        raise ValueError("Observed model drift or missing model audit")
    if result.get("model_mismatch"):
        raise ValueError("Runtime reports model drift")
    tokens = result.get("tokens")
    if not isinstance(tokens, dict) or set(tokens) != set(TOKEN_KEYS):
        raise ValueError("Missing normalized token accounting")
    if any(type(v) is not int or v < 0 for v in tokens.values()):
        raise ValueError("Invalid normalized token count")
    if tokens["input_total"] != tokens["input_uncached"] + tokens["cache_read"] + tokens["cache_write"]:
        raise ValueError("Input token accounting does not reconcile")
    estimated = money(result.get("estimated_cost_usd"))
    reserved = money(result.get("reserved_cost_usd"))
    if estimated + reserved > cap or reserved != 0:
        raise ValueError("Runtime exceeded trial cap or still has unknown outstanding spend")
    if not result["inference_started"] and (estimated or any(tokens.values())):
        raise ValueError("Accounting contradicts the inference-started audit")
    native_cost = result.get("native_reported_cost_usd", result.get("reported_cost_usd", result.get("total_cost_usd")))
    if native_cost is not None:
        native_cost = str(money(native_cost))
    patch = safe_path(artifact.parent / "model.patch", must_exist=True)
    recovery = load(artifact.parent / "recovery.json")
    capture = recovery.get("patch_capture", {})
    if (capture.get("status") != "passed" or capture.get("sha256") != sha256(patch)
            or recovery.get("private_cleanup") != "passed"):
        raise ValueError("Patch capture/private cleanup failed or captured digest differs")
    original_result = artifact.parent.parent / "result.json"
    solver_duration = result.get("duration_seconds")
    if solver_duration is not None and (type(solver_duration) not in (int, float)
                                        or not math.isfinite(solver_duration) or solver_duration < 0):
        raise ValueError("Invalid runtime solver duration")
    return {"runtime_status": result.get("status"), "solver_duration_seconds": solver_duration, "tokens": tokens,
            "accounting_complete": True, "inference_started": result["inference_started"],
            "observed_models": models, "estimated_cost_usd": str(estimated),
            "reserved_cost_usd": str(reserved),
            "native_reported_cost_usd": native_cost,
            "result_artifact": str(artifact.relative_to(ROOT)), "result_sha256": sha256(artifact),
            "patch_artifact": str(patch.relative_to(ROOT)), "patch_sha256": sha256(patch),
            "recovery_artifact": str((artifact.parent / "recovery.json").relative_to(ROOT)),
            "recovery_sha256": sha256(artifact.parent / "recovery.json"),
            "original_result_artifact": str(original_result.relative_to(ROOT)) if original_result.is_file() else None,
            "original_result_sha256": sha256(original_result) if original_result.is_file() else None,
            "original_grade": grade(original_result)}


def harbor_environment(config_path):
    # Preserve only host-path information required by the explicitly authorized
    # credential resolver; never forward unrelated provider credentials/proxies.
    allowed = ("PATH", "HOME", "USER", "LOGNAME", "TMPDIR", "LANG", "LC_ALL", "LC_CTYPE", "TZ",
               "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME", "XDG_CACHE_HOME", "XDG_RUNTIME_DIR",
               "NEOISM_AGENT_AUTH_PATH", "DOCKER_HOST", "DOCKER_CONTEXT")
    environment = {key: os.environ[key] for key in allowed if key in os.environ}
    docker_config = safe_path(Path(config_path).parent / "private-docker-config")
    docker_config.mkdir(mode=0o700, exist_ok=True)
    environment.update(PYTHONPATH=str(ROOT), PYTHONNOUSERSITE="1", DOCKER_CONFIG=str(docker_config),
                       NEOISM_PUBLIC_EVAL_ALLOW_INFERENCE=os.environ.get("NEOISM_PUBLIC_EVAL_ALLOW_INFERENCE", "0"))
    return environment


def harbor(config_path, log_path):
    # Invoke the installed native Harbor CLI, not a custom provider/model loop.
    environment = harbor_environment(config_path)
    with log_path.open("xb") as log:
        os.chmod(log_path, 0o600)
        try:
            process = subprocess.run([str(ROOT / ".venv/bin/harbor"), "run", "--config", str(config_path), "--yes"],
                                     cwd=ROOT, env=environment, stdout=log, stderr=subprocess.STDOUT,
                                     timeout=7200, check=False)
            return process.returncode
        except subprocess.TimeoutExpired:
            # A native timeout is not a retry. Accounting/capture are still required.
            return "runner_timeout"


def execute(plan, protocol, plan_sha, approval_path, evidence_path, *, execute_requested=False):
    if execute_requested is not True:
        raise ValueError("Execution requires the explicit --execute gate")
    if os.environ.get("NEOISM_PUBLIC_EVAL_ALLOW_INFERENCE") != "1":
        raise ValueError("Execution also requires NEOISM_PUBLIC_EVAL_ALLOW_INFERENCE=1")
    approval_path = safe_path(approval_path, must_exist=True)
    approval, maximum, per_trial = validate_approval(approval_path, plan, protocol, plan_sha)
    approval_sha = sha256(approval_path)
    validate_readiness(evidence_path, plan, plan_sha)
    with locked_ledger() as ledger:
        if ledger["blocked"] or any(e["state"] != "complete" for e in ledger["entries"].values()):
            raise ValueError("Inference blocked: prior dispatched trial interrupted or unresolved; full cap remains reserved")
        experiment = ledger["experiments"].get(plan_sha)
        if experiment and experiment["approval_sha256"] != approval_sha:
            raise ValueError("Cannot replace an experiment's approved budget")
        ledger["experiments"].setdefault(plan_sha, {"approval_sha256": approval_sha, "created_at": now()})
        # The cap is across the fixed ledger, not a fresh budget for each CLI entry.
        charged = sum((money(e["charged_cost_usd"]) for e in ledger["entries"].values()), Decimal(0))
        for slot in plan["trials"]:
            key = identity(slot, protocol)
            if key in ledger["entries"]:
                previous = ledger["entries"][key]
                if previous["plan_sha256"] != plan_sha:
                    raise ValueError("Task/harness already dispatched under another experiment; no repeats")
                continue  # only complete entries are eligible for skipping
            if any(e["slot"] == slot["slot"] for e in ledger["entries"].values()):
                raise ValueError("This slot was already dispatched under another experiment; no repeats")
            # Revalidate source/config/artifact integrity and expiry immediately before admission.
            _, _, current_sha = checked_plan()
            if current_sha != plan_sha or sha256(approval_path) != approval_sha:
                raise ValueError("Plan or approval changed during invocation")
            validate_approval(approval_path, plan, protocol, plan_sha)
            validate_readiness(evidence_path, plan, plan_sha)
            if charged + per_trial > maximum:
                atomic_json(LEDGER, ledger)
                return {"status": "budget_exhausted", "next_untouched_slot": slot["slot"]}
            source = checked_config(slot, protocol)
            replay = checked_config(slot, protocol, regrade=True)
            source_job, replay_job = job_path(source), job_path(replay)
            if source_job.exists() or replay_job.exists() or any(e["source_job"] == str(source_job) for e in ledger["entries"].values()):
                raise ValueError("Refusing existing or duplicate Harbor job directory")
            private = safe_path(RUNS / "admissions" / plan_sha / slot["slot"])
            if private.exists():
                raise ValueError("Prior admission artifacts exist; do not repeat this slot")
            private.mkdir(parents=True, mode=0o700)
            admission_path = private / "admission.json"
            atomic_json(admission_path, {
                "slot": slot["slot"], "plan_sha256": plan_sha,
                "maximum_api_cost_usd": float(per_trial), "approval_sha256": approval_sha,
                "issued_at": now(), "ledger_path": str(LEDGER),
                "source_config_sha256": sha256(safe_path(slot["job_config"], must_exist=True)),
                "runner_pid": os.getpid()})
            source["agents"][0]["kwargs"].update(authorization_path=str(approval_path), admission_path=str(admission_path))
            runtime_config = private / "job.json"
            atomic_json(runtime_config, source)
            entry = {"key": key, "slot": slot["slot"], "task": slot["task"], "harness": slot["harness"],
                     "plan_sha256": plan_sha, "approval_sha256": approval_sha,
                     "source_job": str(source_job), "regrade_job": str(replay_job),
                     "maximum_api_cost_usd": str(per_trial), "charged_cost_usd": str(per_trial),
                     "state": "in_flight", "dispatched_at": now(), "runner_pid": os.getpid(),
                     "admission_path": str(admission_path), "admission_sha256": sha256(admission_path),
                     "source_config_sha256": sha256(safe_path(slot["job_config"], must_exist=True))}
            ledger["entries"][key] = entry
            # This fsync precedes subprocess creation. Crash = full reservation + permanent block.
            atomic_json(LEDGER, ledger)
            started = time.monotonic()
            try:
                entry["source_returncode"] = harbor(runtime_config, private / "source.log")
                summary = collect_source(source_job, protocol["model"]["id"], per_trial)
                entry.update(summary)
                # Keep full reservation until both capture and pristine grading finish.
                atomic_json(LEDGER, ledger)
                if sha256(safe_path(summary["patch_artifact"], must_exist=True)) != summary["patch_sha256"]:
                    raise ValueError("Captured patch changed before pristine replay")
                _, _, current_sha = checked_plan()
                if current_sha != plan_sha:
                    raise ValueError("Protected sources/config changed before pristine regrade")
                entry["regrade_returncode"] = harbor(safe_path(slot["regrade_config"], must_exist=True), private / "regrade.log")
                pristine_result = one_trial_result(replay_job)
                replay_audit_path = pristine_result.parent / "agent/replay.json"
                replay_audit = load(replay_audit_path)
                if (replay_audit.get("task") != slot["task"]
                        or replay_audit.get("patch_sha256") != summary["patch_sha256"]
                        or replay_audit.get("status") not in {"applied", "empty"}
                        or replay_audit.get("inference_calls") != 0
                        or replay_audit.get("credentials_read") is not False):
                    raise ValueError("Pristine replay audit missing or differs from captured patch/task")
                entry["replay_audit_artifact"] = str(replay_audit_path.relative_to(ROOT))
                entry["replay_audit_sha256"] = sha256(replay_audit_path)
                entry["pristine_grade"] = grade(pristine_result)
                if entry["regrade_returncode"] != 0:
                    entry["pristine_grade"] = {"status": "infra_error", "reward": None,
                                               "reported_reward": entry["pristine_grade"].get("reward")}
                entry["pristine_result_artifact"] = str(pristine_result.relative_to(ROOT))
                entry["pristine_result_sha256"] = sha256(pristine_result)
                if entry["pristine_grade"]["status"] == "infra_error":
                    raise ValueError("Pristine verifier infrastructure error (not a failed solve)")
                entry["state"] = "complete"
                entry["charged_cost_usd"] = str(money(summary["estimated_cost_usd"]) + money(summary["reserved_cost_usd"]))
                charged += money(entry["charged_cost_usd"])
                entry["finished_at"] = now()
                entry["orchestration_duration_seconds"] = time.monotonic() - started
                atomic_json(LEDGER, ledger)
            except BaseException as exc:
                entry["state"] = "blocked"
                entry["charged_cost_usd"] = str(per_trial)
                entry["error"] = str(exc)
                entry["orchestration_duration_seconds"] = time.monotonic() - started
                ledger["blocked"] = {"slot": slot["slot"], "reason": str(exc), "at": now()}
                atomic_json(LEDGER, ledger)
                raise
        return {"status": "complete", "trials": 153, "reserved_cost_usd": str(charged)}


def artifact_errors(entry):
    errors = []
    for prefix in ("result", "patch", "recovery", "pristine_result", "replay_audit"):
        path = entry.get(prefix + "_artifact")
        expected = entry.get(prefix + "_sha256")
        try:
            if not path or not expected or sha256(safe_path(path, must_exist=True)) != expected:
                errors.append(prefix)
        except (ValueError, OSError):
            errors.append(prefix)
    original = entry.get("original_result_artifact")
    if original:
        try:
            if sha256(safe_path(original, must_exist=True)) != entry.get("original_result_sha256"):
                errors.append("original_result")
        except (ValueError, OSError):
            errors.append("original_result")
    return errors


def build_report(plan, protocol, plan_sha, ledger):
    entries = ledger["entries"]
    trials = []
    groups = {}
    pairing = {}
    for slot in plan["trials"]:
        entry = entries.get(identity(slot, protocol))
        trial: dict = dict(entry) if entry else dict(slot, state="untouched")
        trial["artifact_integrity_errors"] = artifact_errors(entry) if entry else []
        trials.append(trial)
        group = groups.setdefault(slot["harness"], {"planned": 51, "complete": 0, "untouched": 0,
            "blocked_or_in_flight": 0, "pristine_pass": 0, "pristine_fail": 0,
            "pristine_infra_error": 0, "original_pass": 0, "original_fail": 0, "original_infra_error": 0,
            "orchestration_duration_seconds": 0, "solver_duration_seconds": 0, "solver_duration_missing": 0,
            "artifact_integrity_errors": 0, "tokens": dict.fromkeys(TOKEN_KEYS, 0),
            "estimated_cost_usd": Decimal(0), "reserved_cost_usd": Decimal(0),
            "native_reported_cost_usd": Decimal(0), "native_cost_missing": 0, "observed_models": set()})
        if entry is None:
            group["untouched"] += 1
            pairing.setdefault(slot["task"], {})[slot["harness"]] = "untouched"
            continue
        valid = entry["state"] == "complete" and not trial["artifact_integrity_errors"] and entry.get("plan_sha256") == plan_sha
        group["complete" if valid else "blocked_or_in_flight"] += 1
        group["artifact_integrity_errors"] += bool(trial["artifact_integrity_errors"])
        group["orchestration_duration_seconds"] += entry.get("orchestration_duration_seconds", 0)
        if entry.get("solver_duration_seconds") is None:
            group["solver_duration_missing"] += 1
        else:
            group["solver_duration_seconds"] += entry["solver_duration_seconds"]
        group["reserved_cost_usd"] += money(entry["charged_cost_usd"])
        group["estimated_cost_usd"] += money(entry.get("estimated_cost_usd", 0))
        native = entry.get("native_reported_cost_usd")
        if native is None:
            group["native_cost_missing"] += 1
        else:
            group["native_reported_cost_usd"] += money(native)
        group["observed_models"].update(entry.get("observed_models", []))
        for key in TOKEN_KEYS:
            group["tokens"][key] += entry.get("tokens", {}).get(key, 0)
        for kind in ("original", "pristine"):
            status = entry.get(kind + "_grade", {}).get("status", "infra_error") if not trial["artifact_integrity_errors"] else "infra_error"
            group[kind + "_" + {"passed": "pass", "failed": "fail", "infra_error": "infra_error"}[status]] += 1
        pairing.setdefault(slot["task"], {})[slot["harness"]] = entry.get("pristine_grade", {}).get("status", "infra_error") if valid else "infra_error"
    discordances = [{"task": task, "pristine_status_by_harness": statuses}
                    for task, statuses in pairing.items()
                    if len(statuses) != 3 or len(set(statuses.values())) != 1]
    for group in groups.values():
        group["observed_models"] = sorted(group["observed_models"])
        for key in ("estimated_cost_usd", "reserved_cost_usd", "native_reported_cost_usd"):
            group[key] = str(group[key])
    complete = (len(trials) == 153 and len({t["task"] for t in trials}) == 51
                and len(groups) == 3 and not ledger.get("blocked")
                and all(t["state"] == "complete" and t.get("accounting_complete") is True
                        and t.get("plan_sha256") == plan_sha and not t["artifact_integrity_errors"]
                        and t.get("pristine_grade", {}).get("status") in {"passed", "failed"} for t in trials))
    catalog_path = ROOT / "model-catalog.lock.json"
    catalog = load(catalog_path) if catalog_path.exists() else {}
    return {"schema_version": 1, "plan_sha256": plan_sha, "hard51_paired_subset_complete": complete,
            "full_benchmark_complete": False, "dataset_scope": "SWE-bench Pro V2 HARD-51 subset of 642 tasks",
            "catalog": {"sha256": catalog.get("sha256"), "pricing": catalog.get("model", {}).get("cost"),
                        "model": plan["model"], "lock_sha256": plan.get("catalog_lock_sha256")},
            "claim": "HARD-51 paired SUBSET complete (not the full 642-task benchmark)" if complete else "INCOMPLETE HARD-51 SUBSET - not a complete paired subset or full-benchmark win",
            "authoritative_score": "pristine verifier reward only; infrastructure errors are not failures",
            "budget_semantics": "Conservative frozen-catalog reservation, not provider invoice guarantee",
            "harness_totals": groups, "task_pairings": pairing, "pairing_discordances": discordances,
            "incomplete_task_pairings": [task for task, statuses in pairing.items()
                                         if len(statuses) != 3 or not set(statuses.values()) <= {"passed", "failed"}],
            "trials": trials,
            "ledger_blocked": ledger.get("blocked")}


def write_report(report):
    atomic_json(RUNS / "report.json", report)
    lines = ["# Public HARD-51 report", "", report["claim"], "", report["budget_semantics"], "",
             "Authoritative success = known pristine verifier pass; infrastructure errors are separate.", "",
             "| Harness | Complete/51 | Pristine pass/fail/infra | Original pass/fail/infra | Solver seconds | Orchestration seconds | Estimated $ | Reserved $ | Native reported $ |",
             "|---|---:|---|---|---:|---:|---:|---:|---:|"]
    for name, group in report["harness_totals"].items():
        lines.append(f"| {name} | {group['complete']}/51 | {group['pristine_pass']}/{group['pristine_fail']}/{group['pristine_infra_error']} | "
                     f"{group['original_pass']}/{group['original_fail']}/{group['original_infra_error']} | {group['solver_duration_seconds']:.1f} (missing {group['solver_duration_missing']}) | {group['orchestration_duration_seconds']:.1f} | "
                     f"{group['estimated_cost_usd']} | {group['reserved_cost_usd']} | {group['native_reported_cost_usd']} (missing {group['native_cost_missing']}) |")
    lines.extend(["", "## Token/cache accounting and observed models", "", "```json",
                  json.dumps({k: {"tokens": v["tokens"], "observed_models": v["observed_models"]} for k, v in report["harness_totals"].items()}, indent=2),
                  "```", "", "## Task pairing discordances", "", "```json",
                  json.dumps(report["pairing_discordances"], indent=2), "```", "",
                  "Per-trial raw artifact paths, hashes, accounting and both grades are in report.json."])
    # Markdown is derivative; JSON and the fsync ledger remain authoritative.
    destination = RUNS / "report.md"
    fd, temporary = tempfile.mkstemp(prefix=".report-", dir=RUNS)
    try:
        with os.fdopen(fd, "w") as stream:
            stream.write("\n".join(lines) + "\n")
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, destination)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=["plan", "status", "report", "approval-template", "run"])
    parser.add_argument("--execute", action="store_true")
    parser.add_argument("--approval", type=Path)
    parser.add_argument("--readiness", type=Path)
    args = parser.parse_args()
    if args.execute and args.operation != "run":
        parser.error("--execute is only valid with run")
    if args.operation != "run" and (args.approval or args.readiness):
        parser.error("Approval/readiness overrides are only valid with run")
    plan, protocol, plan_sha = checked_plan()
    if args.operation == "approval-template":
        print(json.dumps(approval_template(plan, protocol, plan_sha), indent=2))
    elif args.operation == "plan":
        print(json.dumps({"plan_sha256": plan_sha, "trials": 153, "inference": False, "plan": plan}, indent=2))
    elif args.operation in {"status", "report"}:
        with locked_ledger() as ledger:
            report = build_report(plan, protocol, plan_sha, ledger)
            if args.operation == "report":
                write_report(report)
            print(json.dumps({k: v for k, v in report.items() if k != "trials"}, indent=2))
    elif not args.execute:
        print(json.dumps({"inference": False, "trials": 153, "plan_sha256": plan_sha,
                          "required": "--execute AND NEOISM_PUBLIC_EVAL_ALLOW_INFERENCE=1 AND --approval AND --readiness"}))
    else:
        if not args.approval or not args.readiness:
            parser.error("--execute requires --approval and --readiness")
        try:
            result = execute(plan, protocol, plan_sha, args.approval, args.readiness, execute_requested=args.execute)
        finally:
            if LEDGER.exists():
                with locked_ledger() as ledger:
                    write_report(build_report(plan, protocol, plan_sha, ledger))
        print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
