#!/usr/bin/env python3
"""Explicit credential-free infrastructure lab. No operation enables inference.

Without --execute, only describe the selected operation. All executed evidence
uses a unique directory and binds the immutable plan, configs and tool sources.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import importlib
import json
import math
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import tempfile
import unittest
import uuid

import prepare as prep

ROOT = prep.ROOT
OPERATIONS = ("unit", "wire-tests", "network-probe", "install-only", "calibration", "pristine-replay", "image-metadata", "pull-images")


def scrub_environment(home):
    """Allowlist, not denylist: never copy credentials/inference switches."""
    home = Path(home)
    home.mkdir(parents=True, exist_ok=True)
    env = {key: os.environ[key] for key in ("PATH", "LANG", "LC_ALL", "TZ") if key in os.environ}
    env.update(HOME=str(home), XDG_CONFIG_HOME=str(home / "config"),
               XDG_CACHE_HOME=str(home / "cache"), XDG_DATA_HOME=str(home / "data"),
               DOCKER_CONFIG=str(home / "docker"), PYTHONPATH=str(ROOT),
               PYTHONNOUSERSITE="1", GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL="/dev/null")
    return env


def verify_plan(path=None):
    """Read-only API for readiness/runner: raises on any immutable input drift."""
    path = prep.regular_file(path or ROOT / "plan.json")
    plan = json.loads(path.read_text())
    if plan.get("schema_version") != 2:
        raise ValueError("Regenerate the immutable schema-2 plan")
    bindings = {"protocol_sha256": "protocol.json", "dataset_lock_sha256": "hard51.lock.json",
                "assets_lock_sha256": "assets.lock.json", "images_lock_sha256": "images.lock.json",
                "catalog_lock_sha256": "model-catalog.lock.json"}
    for key, file in bindings.items():
        if prep.digest(prep.regular_file(ROOT / file)) != plan[key]:
            raise ValueError("Plan input changed: " + file)
    if prep.source_hashes() != plan["source_hashes"] or prep.source_lock_hashes() != plan["source_lock_hashes"]:
        raise ValueError("Public Python implementation/dependency locks changed; regenerate plan and evidence")
    if prep.digest(prep.regular_file(ROOT.parent / "saved_auth.py")) != plan["credential_resolver_sha256"]:
        raise ValueError("Credential resolver source changed (source hash only; no auth reads)")
    prep.validate_assets()
    prep.validate_catalog()
    if prep.digest(prep.regular_file(prep.CHECKOUT / "v2/tooling/patch_replay.py")) != plan["upstream_replay_sha256"]:
        raise ValueError("Pinned upstream replay source changed")
    tasks = json.loads((ROOT / "hard51.lock.json").read_text())["tasks"]
    prep.validate_images(tasks)
    if len(plan["trials"]) != 153:
        raise ValueError("Expected exactly 153 planned trials")
    for slot in plan["trials"]:
        for key in ("job_config", "regrade_config"):
            config = ROOT / slot[key]
            if not config.resolve().is_relative_to(ROOT) or prep.digest(prep.regular_file(config)) != slot[key + "_sha256"]:
                raise ValueError("Planned config changed")
    return plan


def read_evidence(path, plan_path=None, operation=None):
    """Readiness API: validate a successful report and every recorded artifact.

    Report schema: schema_version, operation, evidence_id, started_at/finished_at,
    plan_sha256, source_hashes, config_hashes, artifact_hashes, jobs, status,
    inference_calls=0, credentials_read=False. Paths in hashes are report-relative.
    Calibration jobs contain task, expected_reward, observed_reward, source_job.
    This validates ONE batch, not full51 readiness. For an aggregated check,
    validate each evidence reference and union its jobs/coverage; do not inspect
    only the first reference or assume every individual batch covers all51.
    """
    plan = verify_plan(plan_path)
    path = prep.regular_file(path)
    report = json.loads(path.read_text())
    if (report.get("schema_version") != 1 or report.get("status") != "passed" or
            report.get("inference_calls") != 0 or report.get("credentials_read") is not False or
            report.get("plan_sha256") != prep.digest(Path(plan_path or ROOT / "plan.json")) or
            report.get("source_hashes") != plan["source_hashes"] or
            (operation and report.get("operation") != operation) or
            report.get("evidence_id") != path.parent.name):
        raise ValueError("Missing, stale, failed or incompatible lab evidence")
    for group in ("config_hashes", "artifact_hashes"):
        for name, expected in report[group].items():
            artifact = path.parent / name
            if not artifact.resolve().is_relative_to(path.parent) or prep.digest(prep.regular_file(artifact)) != expected:
                raise ValueError("Evidence artifact changed: " + name)
    return report


def base_config(directory, name, task, agent, verify=True):
    return {"job_name": name, "jobs_dir": str(directory / "jobs"),
            "n_attempts": 1, "n_concurrent_trials": 1, "retry": {"max_retries": 0},
            "environment": {"type": "docker"}, "verifier": {"disable": not verify},
            "agents": [agent], "tasks": [{"path": str(task)}]}


def trial_result(job, task, verify=True, expected=None):
    results = list(job.glob("*/result.json"))
    if len(results) != 1:
        raise RuntimeError("Expected exactly one completed trial result")
    result = json.loads(prep.regular_file(results[0]).read_text())
    if Path(result["task_name"]).name != task or result.get("exception_info") or not result.get("finished_at"):
        raise RuntimeError("Trial infrastructure failure/incomplete result: " + str(results[0]))
    exit_code = results[0].parent / "agent/exit-code.txt"
    if exit_code.exists() and prep.regular_file(exit_code).read_text().strip() != "0":
        raise RuntimeError("Oracle solve.sh failed")
    reward = None
    if verify:
        rewards = (result.get("verifier_result") or {}).get("rewards")
        if not isinstance(rewards, dict) or set(rewards) != {"reward"} or type(rewards["reward"]) not in (int, float) or rewards["reward"] not in (0, 1):
            raise RuntimeError("Verifier omitted an unambiguous binary reward; not a false result")
        reward = rewards["reward"]
        if expected is not None and reward != expected:
            raise RuntimeError("Calibration disagrees with official oracle=1 / nop=0 semantics")
    return reward


def require_container_reserve(config, env, reserve_gib):
    """No implicit image downloads by probe/calibration jobs; explicit pull first."""
    task = Path(config["tasks"][0]["path"]).name
    lock = json.loads((ROOT / "hard51.lock.json").read_text())
    images = prep.validate_images(lock["tasks"])
    image = next(item for item in images["images"] if item["task"] == task)
    task_spec = next(item for item in lock["tasks"] if item["id"] == task)
    inspected = subprocess.run(["docker", "image", "inspect", image["image"]],
                               env=env, capture_output=True, text=True, timeout=30)
    if inspected.returncode:
        raise ValueError("Pinned image not local; use explicit budgeted pull-images first (no automatic job pulls)")
    info = json.loads(subprocess.check_output(["docker", "info", "--format", "{{json .}}"], env=env, text=True, timeout=30))
    storage_mb = task_spec["resources"].get("storage_mb")
    if type(storage_mb) not in (int, float) or storage_mb <= 0:
        raise ValueError("Unknown task disk allowance")
    required = reserve_gib * 1024**3 + storage_mb * 1024**2
    for disk in (Path(info["DockerRootDir"]), ROOT):
        if shutil.disk_usage(disk).free < required:
            raise ValueError("Insufficient disk reserve plus task storage allowance")


def isolated_worker_command(worker):
    # No fallback: unsupported user/network namespaces block infrastructure tests.
    return ["unshare", "--user", "--map-root-user", "--net", "--", "sh", "-eu", "-c",
            'ip link set lo up; exec "$@"', "public-lab-netns",
            str(ROOT / ".venv/bin/python"), str(ROOT / "lab.py"), worker, "--execute"]


def execute_job(directory, config, env, report, expected=None):
    require_container_reserve(config, env, report["disk_reserve_gib"])
    from harbor.models.job.config import JobConfig
    JobConfig.model_validate(config)
    config_path = directory / "configs" / (config["job_name"] + ".json")
    prep.output_json(config_path, config)
    report["config_hashes"][str(config_path.relative_to(directory))] = prep.digest(config_path)
    job = Path(config["jobs_dir"]) / config["job_name"]
    if job.exists():
        raise ValueError("Lab never resumes/reuses a job directory")
    with (directory / (config["job_name"] + ".log")).open("w") as log:
        subprocess.run([str(ROOT / ".venv/bin/harbor"), "run", "--config", str(config_path), "--yes"],
                       cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT, check=True, timeout=10800)
    task = Path(config["tasks"][0]["path"]).name
    reward = trial_result(job, task, verify=not config["verifier"]["disable"], expected=expected)
    record = {"task": task, "source_job": str(job), "expected_reward": expected, "observed_reward": reward}
    report["jobs"].append(record)
    return record


def fixture_budget(module):
    original = getattr(module, "request", None)
    if not callable(original):
        return
    module.MODELS_CATALOG = prep.CACHE / "models.json"
    def request(*args, **kwargs):
        value = original(*args, **kwargs)
        if not isinstance(value, dict):
            raise TypeError("Native fixture request factory must return a dictionary")
        value.setdefault("catalog_path", str(prep.CACHE / "models.json"))
        value.setdefault("budget", {"maximum_api_cost_usd": 100, "context_limit": 200000,
                                   "pricing": {"input": 1, "output": 1, "cache_read": 1,
                                               "cache_write": 1, "cache_write_1h": 1}})
        return value
    module.request = request


def unit_worker():
    sys.path.insert(0, str(ROOT / "tests"))
    suite = unittest.TestSuite()
    for path in sorted((ROOT / "tests").glob("test_*.py")):
        module = importlib.import_module(path.stem)
        # Do not wrap unrelated gateway/runner request factories: their tests may
        # intentionally omit budgets or assert exact normalized request schemas.
        if module.__name__ in ("test_native_adapters", "test_opencode_v2"):
            fixture_budget(module)
        for value in vars(module).values():
            if (isinstance(value, type) and issubclass(value, unittest.TestCase) and
                    value.__module__ == module.__name__ and
                    value.__name__ not in ("NativeWireFixtures", "NativeSetupTests")):
                suite.addTests(unittest.defaultTestLoader.loadTestsFromTestCase(value))
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    if not result.wasSuccessful() or result.skipped or not result.testsRun:
        raise RuntimeError("Unit checks failed or skipped")


def publish_readiness(report, directory):
    """Atomically union successful same-plan batch manifests and real coverage.

    Each evidence ref remains an individual operation report for read_evidence;
    readers must validate ALL refs and infer coverage from their combined jobs.
    """
    if report["status"] != "passed":
        return
    mapping = {"unit": ["unit"], "wire-tests": ["wire"], "network-probe": ["network"],
               "install-only": ["install"], "calibration": ["oracle", "nop"], "pristine-replay": ["replay"]}
    if report["operation"] not in mapping:
        return
    operations = {name: operation for operation, names in mapping.items() for name in names}
    path = ROOT / "lab-evidence.json"
    document = {"schema_version": 1, "plan_sha256": report["plan_sha256"], "checks": {}}

    def merge(name, observed, reference):
        check = document["checks"].setdefault(name, {"status": "passed", "evidence": [], "coverage": {}})
        if reference not in check["evidence"]:
            check["evidence"].append(reference)
        coverage = {}
        if name in ("oracle", "nop", "replay", "network", "install"):
            expected = 1 if name == "oracle" else 0 if name == "nop" else None
            jobs = [job for job in observed["jobs"] if expected is None or job["expected_reward"] == expected]
            coverage["task_ids"] = {job["task"] for job in jobs}
        if name in ("wire", "install"):
            coverage["harnesses"] = set(prep.PROTOCOL["harnesses"])
        for key, values in coverage.items():
            check["coverage"][key] = sorted(set(check["coverage"].get(key, [])) | values)

    if path.exists():
        previous = json.loads(prep.regular_file(path).read_text())
        if previous.get("schema_version") == 1 and previous.get("plan_sha256") == report["plan_sha256"]:
            for name, check in previous.get("checks", {}).items():
                if name not in operations or check.get("status") != "passed":
                    continue
                for item in check.get("evidence", []):
                    try:
                        if set(item) != {"path", "sha256"} or Path(item["path"]).is_absolute() or ".." in Path(item["path"]).parts:
                            raise ValueError("Unsafe evidence reference")
                        artifact = ROOT / item["path"]
                        if not artifact.resolve().is_relative_to(ROOT) or prep.digest(prep.regular_file(artifact)) != item["sha256"]:
                            raise ValueError("Stale readiness artifact")
                        observed = read_evidence(artifact, operation=operations[name])
                        merge(name, observed, item)
                    except (ValueError, KeyError, OSError, TypeError):
                        pass  # Retain other valid batches, never stale coverage labels.
    evidence = directory / "evidence.json"
    reference = {"path": str(evidence.relative_to(ROOT)), "sha256": prep.digest(evidence)}
    for name in mapping[report["operation"]]:
        merge(name, report, reference)
    prep.output_json(path, document)


def wire_worker(assets):
    # Import only explicit loopback fixtures; no arbitrary test discovery, no paid fallback.
    sys.path.insert(0, str(ROOT / "tests"))
    native = importlib.import_module("test_native_adapters")
    opencode = importlib.import_module("test_opencode_v2")
    fixture_budget(native)
    fixture_budget(opencode)
    opencode.BINARY = Path(assets["agents"]["opencode-v2"]["binary_path"])
    opencode.NativeSetupTests.__unittest_skip__ = False
    suite = unittest.TestSuite()
    suite.addTests(unittest.defaultTestLoader.loadTestsFromTestCase(native.NativeWireFixtures))
    suite.addTests(unittest.defaultTestLoader.loadTestsFromTestCase(opencode.NativeSetupTests))
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    if not result.wasSuccessful() or result.skipped or result.testsRun == 0:
        raise RuntimeError("Wire fixtures failed, skipped or did not execute")


def selected_images(images, task_ids):
    """Filter validated metadata without changing any immutable image pins."""
    chosen = [image for image in images["images"] if image["task"] in set(task_ids)]
    layers = {layer["digest"]: layer["size"] for image in chosen for layer in image.get("layers", [])}
    complete = bool(chosen) and all(image.get("layers") for image in chosen)
    return dict(images, images=chosen, size_metadata_complete=complete,
                unique_compressed_layer_bytes=sum(layers.values()) if complete else None)


def calibration_sources(jobs):
    """A bounded calibration batch must contain both official cases per task."""
    cases = set()
    for job in jobs:
        reward = job.get("expected_reward")
        case = (job.get("task"), reward)
        if (not isinstance(job.get("task"), str) or type(reward) not in (int, float) or
                reward not in (0, 1) or type(job.get("observed_reward")) not in (int, float) or
                job["observed_reward"] != reward or case in cases):
            raise ValueError("Calibration sources require unique successful (task, oracle/nop) pairs")
        cases.add(case)
    if not cases or cases != {(task, reward) for task, _ in cases for reward in (0, 1)}:
        raise ValueError("Every represented calibration task requires BOTH oracle and nop")
    return jobs


def pull_images(images, env, reserve_gib, budget_gib):
    if not images["size_metadata_complete"]:
        raise ValueError("Unknown layer sizes; run image-metadata explicitly before bulk pulls")
    if (not math.isfinite(reserve_gib) or reserve_gib <= 0 or budget_gib is None or
            not math.isfinite(budget_gib) or budget_gib <= 0):
        raise ValueError("Explicit positive --budget-gib and --reserve-gib required")
    required = images["unique_compressed_layer_bytes"]
    if required > budget_gib * 1024**3:
        raise ValueError("Unique compressed layers exceed download budget")
    info = json.loads(subprocess.check_output(["docker", "info", "--format", "{{json .}}"], env=env, text=True, timeout=30))
    docker_root = Path(info["DockerRootDir"])
    # Conservative compressed + unpacked headroom. Never auto-prune anything.
    for image in images["images"]:
        node_bytes = sum({layer["digest"]: layer["size"] for layer in image["layers"]}.values())
        if shutil.disk_usage(docker_root).free < reserve_gib * 1024**3 + node_bytes * 3:
            raise ValueError("Insufficient Docker disk headroom/reserve for current image")
        subprocess.run(["docker", "pull", "--platform", "linux/amd64", image["image"]],
                       check=True, env=env, timeout=7200)
        if shutil.disk_usage(docker_root).free < reserve_gib * 1024**3:
            raise ValueError("Docker disk reserve exhausted; stop without pruning/eviction")


def run_lab(operation, execute=False, plan_path=None, output_root=None, source_job=None,
            source_dir=None, reserve_gib=20, budget_gib=None, task_ids=None):
    if operation not in OPERATIONS:
        raise ValueError("Unknown lab operation")
    if not execute:
        return {"operation": operation, "executed": False, "requires": "--execute",
                "inference_calls": 0, "credentials_read": False}
    if not math.isfinite(reserve_gib) or reserve_gib <= 0:
        raise ValueError("Positive finite --reserve-gib required")
    plan = verify_plan(plan_path)
    directory = (Path(output_root or ROOT / "runs/lab") / (operation + "-" + uuid.uuid4().hex)).absolute()
    if not directory.resolve().is_relative_to(ROOT):
        raise ValueError("Evidence output must be inside public ROOT")
    if any(path.is_symlink() for path in (directory.parent, *directory.parent.parents)):
        raise ValueError("Symlink lab output path")
    directory.mkdir(parents=True, exist_ok=False)
    env = scrub_environment(directory / "private-home")
    report = {"schema_version": 1, "operation": operation, "evidence_id": directory.name,
              "started_at": datetime.now(timezone.utc).isoformat(), "status": "running",
              "plan_sha256": prep.digest(Path(plan_path or ROOT / "plan.json")),
              "source_hashes": plan["source_hashes"], "config_hashes": {}, "artifact_hashes": {},
              "jobs": [], "disk_reserve_gib": reserve_gib,
              "inference_calls": 0, "credentials_read": False}
    try:
        assets = prep.validate_assets()
        lock = prep.verify_dataset()
        if task_ids and operation not in ("network-probe", "install-only", "calibration", "pull-images"):
            raise ValueError("--task is only supported for network-probe, install-only, calibration and pull-images")
        allowed = {task["id"] for task in lock["tasks"]}
        if task_ids and (len(task_ids) != len(set(task_ids)) or not set(task_ids) <= allowed):
            raise ValueError("--task IDs must be distinct members of the immutable HARD-51 plan")
        selected = [task for task in lock["tasks"] if not task_ids or task["id"] in set(task_ids)]
        if operation == "install-only" and not task_ids:
            selected = selected[:1]
        if operation in ("network-probe", "install-only", "calibration", "pull-images"):
            report["selected_task_ids"] = [task["id"] for task in selected]
        subset = prep.materialize_subset(lock)
        if operation == "unit":
            with (directory / "unit.log").open("w") as log:
                subprocess.run(isolated_worker_command("_unit"),
                               env=env, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT, check=True, timeout=1200)
        elif operation == "wire-tests":
            env.update(NEOISM_NATIVE_WIRE_TEST="1",
                       NEOISM_WIRE_BINARY=assets["agents"]["neoism"]["binary_path"],
                       CLAUDE_WIRE_BINARY=assets["agents"]["claude-code"]["binary_path"])
            with (directory / "wire.log").open("w") as log:
                subprocess.run(isolated_worker_command("_wire"),
                               env=env, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT, check=True, timeout=1200)
        elif operation == "network-probe":
            agent = {"import_path": "neoism_harbor.probe:ProtocolProbe", "extra_allowed_hosts": ["api.anthropic.com"]}
            for task in selected:
                execute_job(directory, base_config(directory, "network-" + task["id"], subset / task["id"], agent, False), env, report)
        elif operation == "install-only":
            harnesses = {}
            for name, spec in prep.PROTOCOL["harnesses"].items():
                slot = next(item for item in plan["trials"] if item["harness"] == name)
                harnesses[name] = json.loads((ROOT / slot["job_config"]).read_text())["agents"][0]
            agent = {"import_path": "neoism_harbor.probe:InstallOnlyAgent", "kwargs": {"harnesses": harnesses}}
            for task in selected:
                execute_job(directory, base_config(directory, "install-" + task["id"], subset / task["id"], agent, False), env, report)
        elif operation == "calibration":
            # Official semantics: actually execute solution/solve.sh vs do nothing,
            # then unchanged task verifier. Every selected task, no retry masking.
            for task in selected:
                for mode, expected in (("oracle", 1), ("nop", 0)):
                    agent = ({"import_path": "neoism_harbor.probe:CaptureOracleAgent",
                              "kwargs": {"task_dir": str(subset / task["id"])}}
                             if mode == "oracle" else {"name": "nop"})
                    execute_job(directory, base_config(directory, mode + "-" + task["id"], subset / task["id"], agent), env, report, expected)
        elif operation == "pristine-replay":
            if bool(source_job) == bool(source_dir):
                raise ValueError("Supply exactly one of --source-job or --source-dir")
            if source_dir:
                evidence = read_evidence(Path(source_dir) / "evidence.json", plan_path, "calibration")
                sources = calibration_sources(evidence["jobs"])
            else:
                config = json.loads(prep.regular_file(Path(source_job) / "config.json").read_text())
                if len(config["tasks"]) != 1:
                    raise ValueError("Only one-task source jobs may be replayed")
                sources = [{"task": Path(config["tasks"][0]["path"]).name, "source_job": source_job, "expected_reward": None}]
            report["selected_task_ids"] = sorted({source["task"] for source in sources})
            allowed = {task["id"] for task in lock["tasks"]}
            for index, source in enumerate(sources):
                if source["task"] not in allowed:
                    raise ValueError("Source task not in frozen subset")
                from neoism_harbor.replay import source_patch
                source_patch(source["source_job"], source["task"])
                agent = {"import_path": "neoism_harbor.replay:PatchReplayAgent", "kwargs": {"source_job": str(source["source_job"])}}
                execute_job(directory, base_config(directory, "replay-" + str(index), subset / source["task"], agent), env, report, source["expected_reward"])
            report["source_evidence"] = str(source_dir) if source_dir else None
        elif operation == "image-metadata":
            # Use the same scrubbed environment even for docker's public registry metadata.
            subprocess.run([str(ROOT / ".venv/bin/python"), str(ROOT / "prepare.py"), "images", "--execute"], env=env, cwd=ROOT, check=True, timeout=7200)
            report["plan_regeneration_required"] = True
        elif operation == "pull-images":
            pull_images(selected_images(prep.validate_images(lock["tasks"]), [task["id"] for task in selected]),
                        env, reserve_gib, budget_gib)
        if operation != "image-metadata":
            verify_plan(plan_path)  # Reject implementation/config drift during execution too.
        report["status"] = "passed"
    except Exception as error:
        report.update(status="failed", error=str(error))
        raise
    finally:
        report["finished_at"] = datetime.now(timezone.utc).isoformat()
        # Include configs, results, patches and logs; never the isolated home.
        for path in directory.rglob("*"):
            if path.is_file() and "private-home" not in path.relative_to(directory).parts and path.name != "evidence.json":
                report["artifact_hashes"][str(path.relative_to(directory))] = prep.digest(prep.regular_file(path))
        prep.output_json(directory / "evidence.json", report)
    publish_readiness(report, directory)
    return dict(report, evidence_path=str(directory / "evidence.json"))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=(*OPERATIONS, "_wire", "_unit"))
    parser.add_argument("--execute", action="store_true")
    parser.add_argument("--plan", type=Path)
    parser.add_argument("--output-root", type=Path)
    parser.add_argument("--source-job", type=Path)
    parser.add_argument("--source-dir", type=Path)
    parser.add_argument("--reserve-gib", type=float, default=20)
    parser.add_argument("--budget-gib", type=float)
    parser.add_argument("--task", action="append", dest="task_ids", metavar="ID",
                        help="Repeatable frozen task selection for network/install/calibration/pulls")
    args = parser.parse_args()
    if args.operation in ("_wire", "_unit"):
        if not args.execute:
            parser.error("Internal test workers also require --execute")
        if {name for _, name in socket.if_nameindex()} != {"lo"}:
            parser.error("Test workers require the isolated loopback-only network namespace")
        with tempfile.TemporaryDirectory(prefix="public-lab-worker-") as home:
            clean = scrub_environment(home)
            os.environ.clear()
            os.environ.update(clean)
            assets = prep.validate_assets()
            if args.operation == "_wire":
                os.environ.update(NEOISM_NATIVE_WIRE_TEST="1",
                                  NEOISM_WIRE_BINARY=assets["agents"]["neoism"]["binary_path"],
                                  CLAUDE_WIRE_BINARY=assets["agents"]["claude-code"]["binary_path"])
                wire_worker(assets)
            else:
                unit_worker()
        return
    print(json.dumps(run_lab(args.operation, args.execute, args.plan, args.output_root,
                             args.source_job, args.source_dir, args.reserve_gib, args.budget_gib, args.task_ids), indent=2))


if __name__ == "__main__":
    main()
