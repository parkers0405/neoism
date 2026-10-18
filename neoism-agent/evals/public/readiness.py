#!/usr/bin/env python3
"""Offline, fail-closed readiness evidence validation (never reads credentials)."""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent
CHECKS = {"unit", "wire", "network", "install", "oracle", "nop", "replay"}
MAX_EVIDENCE_BYTES = 32 * 1024 * 1024


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def regular(path, root=ROOT):
    root = Path(root).resolve()
    path = Path(path)
    path = path if path.is_absolute() else root / path
    if ".." in path.parts or not path.is_relative_to(root) or any(p.is_symlink() for p in (path, *path.parents)):
        raise ValueError("Unsafe evidence path: " + str(path))
    if not path.is_file() or path.stat().st_size > MAX_EVIDENCE_BYTES:
        raise ValueError("Missing or oversized evidence: " + str(path))
    return path


def validate_recorded_job(name, job, report, report_path, root):
    """Re-read the hashed config/result, rather than trust job reward labels."""
    directory = Path(report_path).parent
    source = Path(job["source_job"])
    if not source.is_relative_to(directory) or ".." in source.parts or source.is_symlink():
        raise ValueError("Lab job is outside its evidence batch")

    def bound_json(path, inventory):
        path = regular(path, root)
        relative = str(path.relative_to(directory))
        if report.get(inventory, {}).get(relative) != digest(path):
            raise ValueError("Lab job config/result is not in the hashed evidence inventory")
        return json.loads(path.read_text())

    config = bound_json(directory / "configs" / (source.name + ".json"), "config_hashes")
    if (config.get("datasets") or config.get("n_attempts") != 1 or config.get("n_concurrent_trials") != 1
            or config.get("retry") != {"max_retries": 0} or len(config.get("tasks", [])) != 1
            or Path(config["tasks"][0]["path"]).name != job["task"] or len(config.get("agents", [])) != 1
            or Path(config["jobs_dir"]) / config["job_name"] != source):
        raise ValueError("Recorded lab job is not the one-task, one-attempt evidence config")
    agent = config["agents"][0]
    if name in {"oracle", "nop"}:
        if job["expected_reward"] == 1:
            valid_agent = agent.get("import_path") == "neoism_harbor.probe:CaptureOracleAgent"
        else:
            valid_agent = agent.get("name") == "nop" and not agent.get("import_path")
    else:
        expected_class = {"replay": "neoism_harbor.replay:PatchReplayAgent",
                          "network": "neoism_harbor.probe:ProtocolProbe",
                          "install": "neoism_harbor.probe:InstallOnlyAgent"}[name]
        valid_agent = agent.get("import_path") == expected_class
    if not valid_agent:
        raise ValueError("Lab job's actual agent does not match its claimed case")
    paths = list(source.glob("*/result.json"))
    if len(paths) != 1:
        raise ValueError("Lab evidence must have exactly one trial result")
    result = bound_json(paths[0], "artifact_hashes")
    if Path(result.get("task_name", "")).name != job["task"] or result.get("exception_info") or not result.get("finished_at"):
        raise ValueError("Lab result has incomplete/exceptional infrastructure or wrong task")
    if name in {"oracle", "nop", "replay"}:
        rewards = (result.get("verifier_result") or {}).get("rewards")
        if (not isinstance(rewards, dict) or set(rewards) != {"reward"}
                or type(rewards["reward"]) not in (int, float) or rewards["reward"] not in (0, 1)
                or rewards["reward"] != job["expected_reward"] or rewards["reward"] != job["observed_reward"]):
            raise ValueError("Actual verifier reward differs from recorded calibration/replay case")


def infer_coverage(name, report, tasks, harnesses, root, report_path, require_full_replay=True):
    """Infer task/case coverage from immutable operation jobs, not check labels."""
    jobs = report.get("jobs")
    if not isinstance(jobs, list) or len(jobs) > 2048:
        raise ValueError("Invalid lab job evidence")
    if name == "unit":
        if jobs:
            raise ValueError("Unit evidence must not contain inference/task jobs")
        return {}
    if name == "wire":
        if jobs:
            raise ValueError("Wire evidence must be the pinned loopback-only worker")
        # The source-bound wire-tests operation executes all three fixed fixtures;
        # no user-selected harness subset or paid fallback exists in that worker.
        return {"harnesses": harnesses}
    seen_jobs, cases, covered = set(), set(), set()
    installed = set()
    for job in jobs:
        if not isinstance(job, dict) or job.get("task") not in tasks:
            raise ValueError("Lab job references an unplanned task")
        source = job.get("source_job")
        if not isinstance(source, str) or source in seen_jobs:
            raise ValueError("Lab evidence reuses a job directory")
        seen_jobs.add(source)
        validate_recorded_job(name, job, report, report_path, root)
        task = job["task"]
        expected, observed = job.get("expected_reward"), job.get("observed_reward")
        if name in {"oracle", "nop", "replay"}:
            if type(expected) not in (int, float) or expected not in (0, 1) or type(observed) not in (int, float) or observed != expected:
                raise ValueError("Calibration/replay requires actual oracle=1 and nop=0 results")
            case = (task, expected)
            if case in cases:
                raise ValueError("Duplicate calibration/replay task/case")
            cases.add(case)
            if name == "oracle" and expected != 1 or name == "nop" and expected != 0:
                continue
        elif task in covered:
            raise ValueError("Duplicate task in single-pass lab operation")
        covered.add(task)
        if name == "install":
            source_path = Path(source)
            if not source_path.is_relative_to(Path(root).resolve()) or ".." in source_path.parts:
                raise ValueError("Installation job escapes public ROOT")
            artifacts = list(source_path.glob("*/agent/install-only.json"))
            if len(artifacts) != 1:
                raise ValueError("Missing all-harness installation results")
            installed_report = json.loads(regular(artifacts[0], root).read_text())
            if set(installed_report) != harnesses or any(v.get("return_code") != 0 for v in installed_report.values()):
                raise ValueError("Frozen harness installation/version checks failed")
            relative = str(artifacts[0].relative_to(Path(report_path).parent))
            if report.get("artifact_hashes", {}).get(relative) != digest(artifacts[0]):
                raise ValueError("Installation audit is not in the frozen lab artifact inventory")
            installed.update(installed_report)
    replay_tasks = tasks if require_full_replay else {task for task, _ in cases}
    if name == "replay" and (not cases or cases != {(task, reward) for task in replay_tasks for reward in (0, 1)}):
        raise ValueError("Replay needs every oracle AND nop roundtrip on each covered task")
    inferred = {"task_ids": covered}
    if name == "install":
        inferred["harnesses"] = installed
    return inferred


def validate_readiness(path, plan, plan_sha256, root=ROOT):
    """Return verified evidence; coverage is structured, never inferred from status."""
    root = Path(root)
    document = json.loads(regular(path, root).read_text())
    if set(document) != {"schema_version", "plan_sha256", "checks"}:
        raise ValueError("Unexpected readiness fields/overrides")
    if type(document["schema_version"]) is not int or document["schema_version"] != 1 or document["plan_sha256"] != plan_sha256:
        raise ValueError("Readiness is not bound to this plan")
    checks = document["checks"]
    if not isinstance(checks, dict) or set(checks) != CHECKS:
        raise ValueError("All seven readiness checks are required")
    tasks = {s["task"] for s in plan["trials"]}
    harnesses = {s["harness"] for s in plan["trials"]}
    from lab import read_evidence
    operations = {"unit": "unit", "wire": "wire-tests", "network": "network-probe", "install": "install-only",
                  "oracle": "calibration", "nop": "calibration", "replay": "pristine-replay"}
    for name, check in checks.items():
        if (not isinstance(check, dict) or set(check) - {"status", "evidence", "coverage"}
                or check.get("status") != "passed"):
            raise ValueError("Readiness check failed or contains overrides: " + name)
        evidence = check.get("evidence")
        if not isinstance(evidence, list) or not 1 <= len(evidence) <= 16384:
            raise ValueError("Evidence list required: " + name)
        seen = set()
        evidence_bytes = 0
        for item in evidence:
            if not isinstance(item, dict) or set(item) != {"path", "sha256"}:
                raise ValueError("Evidence requires path and sha256 only")
            relative = Path(item["path"])
            if relative.is_absolute() or ".." in relative.parts or str(relative) in seen:
                raise ValueError("Evidence must use distinct ROOT-relative paths")
            seen.add(str(relative))
            artifact = regular(relative, root)
            evidence_bytes += artifact.stat().st_size
            if evidence_bytes > 2 * 1024 * 1024 * 1024:
                raise ValueError("Evidence exceeds per-check 2 GiB bound")
            if digest(artifact) != item["sha256"]:
                raise ValueError("Evidence digest mismatch: " + str(relative))
        coverage = check.get("coverage", {})
        if not isinstance(coverage, dict) or set(coverage) - {"task_ids", "harnesses"}:
            raise ValueError("Unexpected coverage overrides")
        for key, expected in (("task_ids", tasks), ("harnesses", harnesses)):
            values = coverage.get(key, [])
            if (not isinstance(values, list) or any(not isinstance(v, str) for v in values)
                    or len(values) != len(set(values)) or not set(values) <= expected):
                raise ValueError("Invalid coverage: " + name)
        if name in {"oracle", "nop", "network", "replay"} and set(coverage.get("task_ids", [])) != tasks:
            raise ValueError(name + " requires full 51-task coverage")
        if name in {"wire", "install"} and set(coverage.get("harnesses", [])) != harnesses:
            raise ValueError(name + " requires all three harnesses")
        # Installation needs all three harnesses on at least one pinned representative image.
        if name == "install" and not coverage.get("task_ids"):
            raise ValueError("Install evidence needs a representative task/image")
        inferred = {}
        recorded_jobs = set()
        for reference in evidence:
            report_path = regular(reference["path"], root)
            report = read_evidence(report_path, operation=operations[name])
            if report.get("operation") != operations[name] or report.get("plan_sha256") != plan_sha256:
                raise ValueError("Underlying evidence has the wrong operation or plan")
            batch = infer_coverage(name, report, tasks, harnesses, root, require_full_replay=False, report_path=report_path)
            for job in report["jobs"]:
                if job["source_job"] in recorded_jobs:
                    raise ValueError("Evidence references the same lab job more than once")
                recorded_jobs.add(job["source_job"])
            for key, values in batch.items():
                inferred.setdefault(key, set()).update(values)
        if {key: set(values) for key, values in coverage.items() if values} != inferred:
            raise ValueError("Declared coverage differs from actual recorded lab jobs: " + name)
    return document


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence", type=Path, required=True)
    args = parser.parse_args()
    # Plan validation is owned by preparation and must be strictly offline.
    from lab import verify_plan
    verify_plan()
    plan_path = ROOT / "plan.json"
    result = validate_readiness(args.evidence, json.loads(plan_path.read_text()), digest(plan_path))
    print(json.dumps({"ready": True, "plan_sha256": result["plan_sha256"]}))


if __name__ == "__main__":
    main()
