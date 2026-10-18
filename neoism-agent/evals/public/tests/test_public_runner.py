"""Source-only offline safety fixtures. Do not run during benchmark preparation.

Every execution fixture replaces Harbor and provenance/evidence validation; no
Docker, provider, real credential, or native harness is contacted by this suite.
"""
from contextlib import ExitStack
from datetime import datetime, timedelta, timezone
from decimal import Decimal
import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile
import types
import unittest
from unittest.mock import patch

SOURCE = Path(__file__).resolve().parents[1]


def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


readiness = module("public_readiness_fixture", SOURCE / "readiness.py")
with patch.dict(sys.modules, {"readiness": readiness}):
    runner = module("public_runner_fixture", SOURCE / "run.py")


class RunnerSafety(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.runs = self.root / "runs"
        self.stack = ExitStack()
        self.addCleanup(self.stack.close)
        for name, value in (("ROOT", self.root), ("RUNS", self.runs),
                            ("LEDGER", self.runs / "public-ledger.json"),
                            ("LOCK", self.runs / "public-ledger.lock")):
            self.stack.enter_context(patch.object(runner, name, value))
        self.stack.enter_context(patch.dict(os.environ, {"NEOISM_PUBLIC_EVAL_ALLOW_INFERENCE": "1"}))
        self.protocol = {"dataset": {"commit": "dataset-commit"}, "model": {"id": "locked-model"},
                         "harnesses": {"a": {"class": "fixture:A"}}}
        self.slots = []
        for index in range(2):
            slot = {"slot": f"{index + 1:03d}-a", "task": f"task-{index}", "harness": "a", "attempt": 1,
                    "job_config": f".cache/job-{index}.json", "regrade_config": f".cache/regrade-{index}.json"}
            self.slots.append(slot)
            source = {"job_name": "job-" + str(index), "jobs_dir": str(self.runs / "hard51"),
                      "n_attempts": 1, "n_concurrent_trials": 1, "retry": {"max_retries": 0},
                      "environment": {"type": "docker"},
                      "agents": [{"import_path": "fixture:A", "model_name": "anthropic/locked-model",
                                  "extra_allowed_hosts": ["api.anthropic.com"], "kwargs": {}}],
                      "tasks": [{"path": str(self.root / ".cache/hard51" / slot["task"])}]}
            replay = json.loads(json.dumps(source))
            replay["job_name"] += "-regrade"
            replay["agents"] = [{"import_path": "neoism_harbor.replay:PatchReplayAgent", "model_name": "replay",
                                  "kwargs": {"source_job": str(runner.job_path(source))}}]
            runner.atomic_json(self.root / slot["job_config"], source)
            runner.atomic_json(self.root / slot["regrade_config"], replay)
        self.plan = {"trials": self.slots, "model": self.protocol["model"]}
        self.plan_sha = "a" * 64
        self.approval_path = self.root / "approval.json"
        self.approval = runner.approval_template(self.plan, self.protocol, self.plan_sha)
        self.approval.update(approved=True, budget_control_verified=True, maximum_api_cost_usd=20,
                             maximum_trial_api_cost_usd=10,
                             expires_at=(datetime.now(timezone.utc) + timedelta(hours=1)).isoformat())
        runner.atomic_json(self.approval_path, self.approval)
        self.stack.enter_context(patch.object(runner, "checked_plan", return_value=(self.plan, self.protocol, self.plan_sha)))
        self.evidence = self.stack.enter_context(patch.object(runner, "validate_readiness"))
        self.stack.enter_context(patch.object(runner, "frozen_agent_kwargs", return_value={}))
        self.harbor = self.stack.enter_context(patch.object(runner, "harbor", side_effect=self.fake_harbor))

    def fake_harbor(self, config_path, log_path):
        config = runner.load(config_path)
        job = runner.job_path(config)
        trial = job / "trial-one"
        trial.mkdir(parents=True)
        if config["agents"][0]["model_name"] == "replay":
            runner.atomic_json(trial / "result.json", {"finished_at": runner.now(), "verifier_result": {"rewards": {"reward": 1}}, "exception_info": None})
            source = Path(config["agents"][0]["kwargs"]["source_job"])
            runner.atomic_json(trial / "agent/replay.json", {"task": Path(config["tasks"][0]["path"]).name,
                "patch_sha256": runner.sha256(source / "trial-one/agent/model.patch"), "status": "empty",
                "inference_calls": 0, "credentials_read": False})
            return 0
        self.assertTrue(runner.LEDGER.exists())
        state = runner.load(runner.LEDGER)
        self.assertEqual(list(state["entries"].values())[-1]["state"], "in_flight")
        self.assertEqual(list(state["entries"].values())[-1]["charged_cost_usd"], "10")
        kwargs = config["agents"][0]["kwargs"]
        admission_path = Path(kwargs["admission_path"])
        self.assertEqual(admission_path.stat().st_mode & 0o777, 0o600)
        admission = runner.load(admission_path)
        self.assertEqual(admission["runner_pid"], os.getpid())
        self.assertEqual(admission["ledger_path"], str(runner.LEDGER))
        self.assertEqual(admission["maximum_api_cost_usd"], 10)
        self.assertEqual(set(admission), {"slot", "plan_sha256", "maximum_api_cost_usd", "approval_sha256",
                         "issued_at", "ledger_path", "source_config_sha256", "runner_pid"})
        runner.atomic_json(trial / "agent/eval-result.json", self.runtime_result())
        patch_path = trial / "agent/model.patch"
        patch_path.write_text("")  # Empty patches are valid captures, not missing captures.
        runner.atomic_json(trial / "agent/recovery.json", {"patch_capture": {"status": "passed", "sha256": runner.sha256(patch_path)},
                                                          "private_cleanup": "passed"})
        runner.atomic_json(trial / "result.json", {"finished_at": runner.now(), "verifier_result": {"rewards": {"reward": 0}}, "exception_info": None})
        return 0

    def runtime_result(self):
        return {"accounting_complete": True, "inference_started": True, "observed_models": ["locked-model"],
                "tokens": {"input_uncached": 2, "cache_read": 3, "cache_write": 5, "input_total": 10, "output_total": 7},
                "estimated_cost_usd": 2, "reserved_cost_usd": 0, "reported_cost_usd": 1.9, "status": "completed"}

    def execute(self):
        return runner.execute(self.plan, self.protocol, self.plan_sha, self.approval_path, self.root / "evidence.json", execute_requested=True)

    def test_all_three_execution_gates_are_required(self):
        with self.assertRaises(ValueError):
            runner.execute(self.plan, self.protocol, self.plan_sha, self.approval_path, self.root / "evidence.json")
        with patch.dict(os.environ, {"NEOISM_PUBLIC_EVAL_ALLOW_INFERENCE": "0"}):
            with self.assertRaises(ValueError):
                self.execute()
        self.approval["approved"] = False
        runner.atomic_json(self.approval_path, self.approval)
        with self.assertRaises(ValueError):
            self.execute()
        self.harbor.assert_not_called()

    def test_expiry_budget_and_exact_identity_fail_closed(self):
        mutations = [{"maximum_api_cost_usd": "20"}, {"maximum_trial_api_cost_usd": "10"},
                     {"maximum_api_cost_usd": False}, {"maximum_trial_api_cost_usd": 0},
                     {"maximum_trial_api_cost_usd": 21}, {"plan_sha256": "wrong"}, {"model": "wrong"},
                     {"dataset_commit": "wrong"}, {"attempts": True}, {"attempts": 2},
                     {"expires_at": "2000-01-01T00:00:00Z"}, {"expires_at": "2999-01-01T00:00:00"},
                     {"skip_network": True}, {"budget_control_verified": False}]
        for mutation in mutations:
            with self.subTest(mutation=mutation):
                runner.atomic_json(self.approval_path, dict(self.approval, **mutation))
                with self.assertRaises(ValueError):
                    self.execute()
        self.harbor.assert_not_called()

    def test_sequential_original_and_pristine_no_source_repeats(self):
        self.assertEqual(self.execute()["status"], "complete")
        self.assertEqual(self.harbor.call_count, 4)
        ledger = runner.load(runner.LEDGER)
        self.assertTrue(all(e["state"] == "complete" for e in ledger["entries"].values()))
        self.assertEqual([e["charged_cost_usd"] for e in ledger["entries"].values()], ["2", "2"])
        self.assertEqual(self.execute()["status"], "complete")
        self.assertEqual(self.harbor.call_count, 4)
        report = runner.build_report(self.plan, self.protocol, self.plan_sha, ledger)
        self.assertEqual(report["harness_totals"]["a"]["original_fail"], 2)
        self.assertEqual(report["harness_totals"]["a"]["pristine_pass"], 2)
        self.assertEqual(report["harness_totals"]["a"]["tokens"]["cache_write"], 10)
        runner.write_report(report)
        self.assertTrue((self.runs / "report.md").is_file())

    def test_partial_and_timeout_still_regrade_known_accounting(self):
        real_fake = self.fake_harbor
        def timeout_source(config_path, log_path):
            result = real_fake(config_path, log_path)
            config = runner.load(config_path)
            if config["agents"][0]["model_name"] != "replay":
                artifact = runner.job_path(config) / "trial-one/agent/eval-result.json"
                runner.atomic_json(artifact, dict(self.runtime_result(), status="timeout"))
                return "runner_timeout"
            return result
        self.harbor.side_effect = timeout_source
        self.assertEqual(self.execute()["status"], "complete")
        self.assertEqual(self.harbor.call_count, 4)

    def test_unknown_spend_model_drift_and_capture_failure_stop_all(self):
        # Each fixture is isolated because the ledger must never be reset in production.
        summary = self.runtime_result()
        summary["accounting_complete"] = False
        original_fake = self.fake_harbor
        def unknown(config_path, log_path):
            rc = original_fake(config_path, log_path)
            config = runner.load(config_path)
            runner.atomic_json(runner.job_path(config) / "trial-one/agent/eval-result.json", summary)
            return rc
        self.harbor.side_effect = unknown
        with self.assertRaises(ValueError):
            self.execute()
        self.assertEqual(self.harbor.call_count, 1)
        ledger = runner.load(runner.LEDGER)
        entry = next(iter(ledger["entries"].values()))
        self.assertEqual(entry["state"], "blocked")
        self.assertEqual(entry["charged_cost_usd"], "10")
        with self.assertRaises(ValueError):
            self.execute()
        self.assertEqual(self.harbor.call_count, 1)

    def test_process_crash_reservation_blocks_new_invocation(self):
        self.harbor.side_effect = KeyboardInterrupt("simulated process interruption")
        with self.assertRaises(KeyboardInterrupt):
            self.execute()
        ledger = runner.load(runner.LEDGER)
        self.assertEqual(next(iter(ledger["entries"].values()))["charged_cost_usd"], "10")
        with self.assertRaises(ValueError):
            self.execute()
        self.assertEqual(self.harbor.call_count, 1)

    def test_hard_crash_in_flight_ledger_is_never_resumed(self):
        # Simulate durable write followed by SIGKILL, which bypasses finally handlers.
        slot = self.slots[0]
        key = runner.identity(slot, self.protocol)
        config = runner.load(self.root / slot["job_config"])
        runner.atomic_json(runner.LEDGER, {"schema_version": 1, "experiments": {}, "blocked": None,
            "entries": {key: {"key": key, "slot": slot["slot"], "state": "in_flight",
                "source_job": str(runner.job_path(config)), "maximum_api_cost_usd": "10", "charged_cost_usd": "10"}}})
        with self.assertRaises(ValueError):
            self.execute()
        self.harbor.assert_not_called()

    def test_reservation_global_cap_before_each_trial(self):
        self.approval.update(maximum_api_cost_usd=11, maximum_trial_api_cost_usd=10)
        runner.atomic_json(self.approval_path, self.approval)
        self.assertEqual(self.execute()["status"], "budget_exhausted")
        self.assertEqual(self.harbor.call_count, 2)
        self.assertEqual(len(runner.load(runner.LEDGER)["entries"]), 1)
        self.assertFalse((self.runs / "hard51/job-1").exists())

    def test_cross_experiment_cannot_repeat_task_harness(self):
        self.execute()
        self.plan_sha = "b" * 64
        self.approval["plan_sha256"] = self.plan_sha
        runner.atomic_json(self.approval_path, self.approval)
        count = self.harbor.call_count
        with self.assertRaises(ValueError):
            self.execute()
        self.assertEqual(self.harbor.call_count, count)

    def test_existing_source_job_is_not_adopted(self):
        (self.runs / "hard51/job-0").mkdir(parents=True)
        with self.assertRaises(ValueError):
            self.execute()
        self.harbor.assert_not_called()

    def test_config_override_multi_task_retry_and_path_escape_rejected(self):
        source_path = self.root / self.slots[0]["job_config"]
        original = runner.load(source_path)
        overrides = [{"n_attempts": 2}, {"n_concurrent_trials": 2}, {"retry": {"max_retries": 1}},
                     {"tasks": original["tasks"] * 2}, {"datasets": []}, {"job_name": "../escape"}]
        for change in overrides:
            runner.atomic_json(source_path, dict(original, **change))
            with self.subTest(change=change), self.assertRaises(ValueError):
                runner.checked_config(self.slots[0], self.protocol)
        with self.assertRaises(ValueError):
            runner.safe_path(self.root / "../outside")

    def test_source_collection_rejects_unknown_models_capture_and_accounting(self):
        config_path = self.root / self.slots[0]["job_config"]
        config = runner.load(config_path)
        # Build artifacts without invoking the admission-sensitive fake Harbor.
        job = runner.job_path(config)
        agent = job / "trial-one/agent"
        agent.mkdir(parents=True)
        patch_path = agent / "model.patch"
        patch_path.write_text("")
        recovery = {"patch_capture": {"status": "passed", "sha256": runner.sha256(patch_path)}, "private_cleanup": "passed"}
        runner.atomic_json(agent / "recovery.json", recovery)
        runner.atomic_json(agent.parent / "result.json", {"finished_at": runner.now(), "verifier_result": {"rewards": {"reward": 1}}})
        for mutation in ({"observed_models": ["drift"]}, {"accounting_complete": False},
                         {"tokens": {"input_total": 1}}, {"estimated_cost_usd": 11},
                         {"inference_started": None}):
            runner.atomic_json(agent / "eval-result.json", dict(self.runtime_result(), **mutation))
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                runner.collect_source(job, "locked-model", Decimal(10))
        runner.atomic_json(agent / "eval-result.json", self.runtime_result())
        runner.atomic_json(agent / "recovery.json", dict(recovery, private_cleanup="failed"))
        with self.assertRaises(ValueError):
            runner.collect_source(job, "locked-model", Decimal(10))

    def test_pristine_infra_error_distinct_from_failed_solve(self):
        path = self.root / "result.json"
        runner.atomic_json(path, {"finished_at": runner.now(), "verifier_result": {"rewards": {"reward": 0}}})
        self.assertEqual(runner.grade(path), {"status": "failed", "reward": 0})
        runner.atomic_json(path, {"finished_at": runner.now(), "verifier_result": {"rewards": {"reward": 1}}, "exception_info": {"exception_type": "VerifierTimeout"}})
        self.assertEqual(runner.grade(path), {"status": "infra_error", "reward": None})
        self.assertEqual(runner.grade(self.root / "missing"), {"status": "infra_error", "reward": None})

    def test_offline_commands_never_read_approval_or_start_subprocess(self):
        for command in ("plan", "status", "report", "approval-template", "run"):
            with self.subTest(command=command), patch.object(sys, "argv", ["run.py", command]), \
                    patch.object(runner, "validate_approval", side_effect=AssertionError("auth access")), \
                    patch.object(runner.subprocess, "run", side_effect=AssertionError("subprocess")), \
                    patch("builtins.print"):
                runner.main()
        self.harbor.assert_not_called()

    def test_approval_template_is_unapproved_without_any_budget(self):
        value = runner.approval_template(self.plan, self.protocol, self.plan_sha)
        self.assertFalse(value["approved"])
        self.assertFalse(value["budget_control_verified"])
        for name in ("maximum_api_cost_usd", "maximum_trial_api_cost_usd", "expires_at"):
            self.assertIsNone(value[name])

    def test_paid_harbor_environment_drops_unrelated_provider_secrets(self):
        with patch.dict(os.environ, {"OPENAI_API_KEY": "dummy", "ANTHROPIC_API_KEY": "dummy", "HTTPS_PROXY": "dummy",
                                     "NEOISM_AGENT_AUTH_PATH": "/never-open-this-fixture", "HOME": str(self.root)}):
            environment = runner.harbor_environment(self.root / self.slots[0]["job_config"])
        for key in ("OPENAI_API_KEY", "ANTHROPIC_API_KEY", "HTTPS_PROXY"):
            self.assertNotIn(key, environment)
        self.assertEqual(environment["NEOISM_AGENT_AUTH_PATH"], "/never-open-this-fixture")
        self.assertEqual(environment["NEOISM_PUBLIC_EVAL_ALLOW_INFERENCE"], "1")
        self.assertTrue(Path(environment["DOCKER_CONFIG"]).is_dir())

    def test_changed_downloaded_artifact_invalidates_authoritative_totals(self):
        self.execute()
        ledger = runner.load(runner.LEDGER)
        entry = next(iter(ledger["entries"].values()))
        (self.root / entry["patch_artifact"]).write_text("changed")
        report = runner.build_report(self.plan, self.protocol, self.plan_sha, ledger)
        self.assertFalse(report["hard51_paired_subset_complete"])
        self.assertFalse(report["full_benchmark_complete"])
        self.assertIn("patch", report["trials"][0]["artifact_integrity_errors"])
        self.assertEqual(report["harness_totals"]["a"]["complete"], 1)

    def test_flock_excludes_second_writer(self):
        with runner.locked_ledger():
            with self.assertRaises(ValueError):
                with runner.locked_ledger():
                    self.fail("second writer acquired the ledger")


class ReadinessEvidence(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.plan_sha = "a" * 64
        self.plan = {"trials": [{"task": f"task-{t}", "harness": h} for t in range(51) for h in ("a", "b", "c")]}
        evidence = self.root / "proof.json"
        evidence.write_text('{"fixture": "no execution"}')
        self.document = {"schema_version": 1, "plan_sha256": self.plan_sha, "checks": {}}
        self.operations = {"unit": "unit", "wire": "wire-tests", "network": "network-probe",
                           "install": "install-only", "oracle": "calibration", "nop": "calibration", "replay": "pristine-replay"}
        for name in readiness.CHECKS:
            directory = self.root / name
            directory.mkdir()
            jobs = []
            coverage = {}
            if name not in ("unit", "wire"):
                task_ids = ["task-0"] if name == "install" else [f"task-{t}" for t in range(51)]
                coverage["task_ids"] = task_ids
                for task in task_ids:
                    for expected in ((0, 1) if name in ("oracle", "nop", "replay") else (None,)):
                        jobs.append({"task": task, "source_job": str(directory / "jobs" / (task + "-" + str(expected))),
                                     "expected_reward": expected, "observed_reward": expected})
            if name in ("wire", "install"):
                coverage["harnesses"] = ["a", "b", "c"]
            report = {"operation": self.operations[name], "plan_sha256": self.plan_sha, "jobs": jobs, "artifact_hashes": {}}
            if name == "install":
                artifact = Path(jobs[0]["source_job"]) / "trial/agent/install-only.json"
                artifact.parent.mkdir(parents=True)
                artifact.write_text(json.dumps({h: {"return_code": 0} for h in ("a", "b", "c")}))
                report["artifact_hashes"][str(artifact.relative_to(directory))] = readiness.digest(artifact)
            report_path = directory / "evidence.json"
            report_path.write_text(json.dumps(report))
            self.document["checks"][name] = {"status": "passed", "evidence": [
                {"path": str(report_path.relative_to(self.root)), "sha256": readiness.digest(report_path)}], "coverage": coverage}
        self.path = self.root / "readiness.json"
        fake_lab = types.SimpleNamespace(read_evidence=lambda path, **kwargs: json.loads(Path(path).read_text()))
        self.lab_patch = patch.dict(sys.modules, {"lab": fake_lab})
        self.lab_patch.start()
        self.addCleanup(self.lab_patch.stop)
        self.original_job_validator = readiness.validate_recorded_job
        self.job_validation_patch = patch.object(readiness, "validate_recorded_job")
        self.job_validation_patch.start()
        self.addCleanup(self.job_validation_patch.stop)

    def check(self):
        self.path.write_text(json.dumps(self.document))
        return readiness.validate_readiness(self.path, self.plan, self.plan_sha, self.root)

    def test_complete_structured_evidence_accepted(self):
        self.assertEqual(self.check()["plan_sha256"], self.plan_sha)

    def test_boolean_status_does_not_substitute_for_coverage(self):
        for name in ("network", "oracle", "nop"):
            previous = self.document["checks"][name]["coverage"]
            self.document["checks"][name]["coverage"] = {"task_ids": ["task-0"], "harnesses": ["a", "b", "c"]}
            with self.subTest(name=name), self.assertRaises(ValueError):
                self.check()
            self.document["checks"][name]["coverage"] = previous

    def test_incomplete_harness_coverage_rejected(self):
        for name in ("install", "wire"):
            self.document["checks"][name]["coverage"]["harnesses"] = ["a", "b"]
            with self.subTest(name=name), self.assertRaises(ValueError):
                self.check()
            self.document["checks"][name]["coverage"]["harnesses"] = ["a", "b", "c"]

    def test_changed_evidence_wrong_plan_and_overrides_rejected(self):
        self.document["plan_sha256"] = "wrong"
        with self.assertRaises(ValueError):
            self.check()
        self.document["plan_sha256"] = self.plan_sha
        self.document["skip_network"] = True
        with self.assertRaises(ValueError):
            self.check()
        del self.document["skip_network"]
        (self.root / "unit/evidence.json").write_text("changed")
        with self.assertRaises(ValueError):
            self.check()

    def rewrite_report(self, name, change):
        reference = self.document["checks"][name]["evidence"][0]
        path = self.root / reference["path"]
        report = json.loads(path.read_text())
        change(report)
        path.write_text(json.dumps(report))
        reference["sha256"] = readiness.digest(path)

    def test_actual_hashed_verifier_reward_cannot_be_replaced_by_job_label(self):
        directory = self.root / "job-audit"
        source = directory / "jobs/oracle-one"
        result_path = source / "trial/result.json"
        result_path.parent.mkdir(parents=True)
        config_path = directory / "configs/oracle-one.json"
        config_path.parent.mkdir()
        config = {"job_name": source.name, "jobs_dir": str(source.parent), "n_attempts": 1, "n_concurrent_trials": 1,
                  "retry": {"max_retries": 0}, "tasks": [{"path": str(self.root / ".cache/task-0")}],
                  "agents": [{"import_path": "neoism_harbor.probe:CaptureOracleAgent"}]}
        config_path.write_text(json.dumps(config))
        result = {"task_name": "task-0", "finished_at": "2026-01-01T00:00:00Z", "verifier_result": {"rewards": {"reward": 1}}}
        result_path.write_text(json.dumps(result))
        report = {"config_hashes": {str(config_path.relative_to(directory)): readiness.digest(config_path)},
                  "artifact_hashes": {str(result_path.relative_to(directory)): readiness.digest(result_path)}}
        job = {"task": "task-0", "source_job": str(source), "expected_reward": 1, "observed_reward": 1}
        self.original_job_validator("oracle", job, report, directory / "evidence.json", self.root)
        result["verifier_result"]["rewards"]["reward"] = 0
        result_path.write_text(json.dumps(result))
        report["artifact_hashes"][str(result_path.relative_to(directory))] = readiness.digest(result_path)
        with self.assertRaises(ValueError):
            self.original_job_validator("oracle", job, report, directory / "evidence.json", self.root)

    def test_coverage_is_unioned_from_all_verified_batch_reports(self):
        reference = self.document["checks"]["network"]["evidence"][0]
        original_path = self.root / reference["path"]
        report = json.loads(original_path.read_text())
        tail = dict(report, jobs=report["jobs"][25:])
        report["jobs"] = report["jobs"][:25]
        original_path.write_text(json.dumps(report))
        reference["sha256"] = readiness.digest(original_path)
        second = self.root / "network-second/evidence.json"
        second.parent.mkdir()
        second.write_text(json.dumps(tail))
        self.document["checks"]["network"]["evidence"].append({"path": str(second.relative_to(self.root)), "sha256": readiness.digest(second)})
        self.assertEqual(self.check()["plan_sha256"], self.plan_sha)
        tail["operation"] = "unit"
        second.write_text(json.dumps(tail))
        self.document["checks"]["network"]["evidence"][-1]["sha256"] = readiness.digest(second)
        with self.assertRaises(ValueError):
            self.check()

    def test_declared_full_coverage_cannot_hide_missing_recorded_jobs(self):
        self.rewrite_report("network", lambda report: report.update(jobs=report["jobs"][:1]))
        with self.assertRaises(ValueError):
            self.check()

    def test_replay_requires_both_oracle_and_nop_for_each_task(self):
        self.rewrite_report("replay", lambda report: report.update(jobs=[j for j in report["jobs"] if j["expected_reward"] == 1]))
        with self.assertRaises(ValueError):
            self.check()

    def test_wrong_underlying_lab_operation_is_rejected(self):
        self.rewrite_report("network", lambda report: report.update(operation="unit"))
        with self.assertRaises(ValueError):
            self.check()

    def test_evidence_escape_symlink_and_duplicate_coverage_rejected(self):
        evidence = self.document["checks"]["unit"]["evidence"][0]
        original_path = evidence["path"]
        evidence["path"] = "../outside.json"
        with self.assertRaises(ValueError):
            self.check()
        evidence["path"] = "alias.json"
        (self.root / "alias.json").symlink_to(self.root / "proof.json")
        with self.assertRaises(ValueError):
            self.check()
        evidence["path"] = original_path
        self.document["checks"]["oracle"]["coverage"]["task_ids"].append("task-0")
        with self.assertRaises(ValueError):
            self.check()


if __name__ == "__main__":
    unittest.main()
