"""Offline contracts for the pilot runner and real-agent adapters."""

import argparse
import base64
import contextlib
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

import lab
import worker


class RunnerTests(unittest.TestCase):
    def args(self, **overrides):
        defaults = {"allow_paid": True, "budget_usd": 12.0, "trial_budget_usd": 1.0,
                    "arms": ["neoism-sol", "claude-sonnet"], "repeat": 2,
                    "accept_soft_budget": True}
        defaults.update(overrides)
        return argparse.Namespace(**defaults)

    @patch.dict(os.environ, {"OPENAI_API_KEY": "test-openai", "ANTHROPIC_API_KEY": "test-anthropic"}, clear=True)
    def test_paid_gates_and_adaptive_budget(self):
        tasks = [{"id": str(i), "timeout_seconds": 180} for i in range(3)]
        lab.validate_paid(self.args(), tasks)
        for overrides in ({"allow_paid": False}, {"budget_usd": None},
                          {"trial_budget_usd": None},
                          {"accept_soft_budget": False}):
            with self.subTest(overrides=overrides), self.assertRaises(ValueError):
                lab.validate_paid(self.args(**overrides), tasks)

    @patch.dict(os.environ, {}, clear=True)
    def test_missing_credentials_never_load_personal_auth(self):
        with self.assertRaisesRegex(ValueError, "Missing OPENAI_API_KEY"):
            lab.validate_paid(self.args(), [{"id": "example", "timeout_seconds": 180}])

    def test_prior_api_spend_never_prices_subscription_or_ignores_unknown_api(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            lab.write_json(root / "results.json", [
                {"arm": "claude-sonnet", "billing_basis": "api_estimate", "reported_cost_usd": 0.1148},
                {"arm": "neoism-sol", "billing_basis": "subscription", "reported_cost_usd": None},
            ])
            self.assertAlmostEqual(lab.prior_api_spend([root]), 0.1148)
            with self.assertRaises(ValueError):
                lab.prior_api_spend([root, root])
            lab.write_json(root / "results.json", [{"arm": "claude-sonnet", "reported_cost_usd": None}])
            with self.assertRaisesRegex(ValueError, "unknown cost"):
                lab.prior_api_spend([root])

    def test_interrupted_prior_api_trial_cannot_disappear_from_budget(self):
        identity = {"task": "task", "arm": "claude-sonnet", "repeat": 1}
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            lab.write_json(root / "results.json", [])
            ledger = {"api_accounting_complete": False, "in_flight_api_trial": identity,
                      "started_api_trials": [identity], "known_api_spend_usd": 0, "prior_api_spend_usd": 0}
            lab.write_json(root / "budget.json", ledger)
            with self.assertRaisesRegex(ValueError, "interrupted or incomplete"):
                lab.prior_api_spend([root])
            ledger.update(api_accounting_complete=True, in_flight_api_trial=None)
            lab.write_json(root / "budget.json", ledger)
            with self.assertRaisesRegex(ValueError, "missing its result"):
                lab.prior_api_spend([root])
            lab.write_json(root / "results.json", [dict(identity, reported_cost_usd=0.2)])
            with self.assertRaisesRegex(ValueError, "disagree"):
                lab.prior_api_spend([root])
            ledger["known_api_spend_usd"] = 0.2
            lab.write_json(root / "budget.json", ledger)
            self.assertAlmostEqual(lab.prior_api_spend([root]), 0.2)

    def test_prior_run_ancestry_is_counted_once(self):
        with tempfile.TemporaryDirectory() as directory:
            first, second = Path(directory) / "first", Path(directory) / "second"
            first.mkdir()
            second.mkdir()
            lab.write_json(first / "results.json", [{"arm": "claude-sonnet", "reported_cost_usd": 0.1}])
            lab.write_json(second / "results.json", [{"arm": "claude-sonnet", "reported_cost_usd": 0.2}])
            lab.write_json(second / "manifest.json", {"prior_runs": [str(first)]})
            self.assertAlmostEqual(lab.prior_api_spend([second]), 0.3)
            self.assertAlmostEqual(lab.prior_api_spend([first, second]), 0.3)

    def test_subscription_timeout_counts_as_failure_without_stopping_other_trials(self):
        task = lab.load_tasks()[0]
        attempts = []
        def fake_trial(image, output, selected, arm, repeat, *args):
            if arm == "baseline":
                return {"status": "completed", "passed": False, "verification": {"valid": True,
                    "returncode": 1, "outcome": {"tests_run": 10, "failures": 1, "errors": 0, "skipped": 0}}}
            if arm == "oracle":
                return {"passed": True}
            attempts.append(repeat)
            return {"status": "timeout", "passed": False, "reported_cost_usd": None,
                    "billing_basis": "subscription", "accounting_complete": False}
        args = self.args(budget_usd=20, trial_budget_usd=5, arms=["neoism-sol"], repeat=2,
                         task=None, image="image", seed=0, effort="high", suite="smoke")
        with tempfile.TemporaryDirectory() as directory, contextlib.redirect_stdout(io.StringIO()), \
             patch("lab.load_tasks", return_value=[task]), patch("lab.image_id", return_value="image-id"), \
             patch("lab.new_output", return_value=Path(directory)), patch("lab.preflight"), \
             patch("lab.provenance", return_value={}), patch("lab.report"), \
             patch("lab.resolve_credential", return_value={"type": "oauth", "access": "synthetic-test-access"}), \
             patch("lab.trial", side_effect=fake_trial):
            lab.run(args)
            ledger = json.loads((Path(directory) / "budget.json").read_text())
        self.assertEqual(len(attempts), 2)
        self.assertEqual(ledger["known_api_spend_usd"], 0)
        self.assertTrue(ledger["api_accounting_complete"])

    def test_per_trial_api_cap_is_clipped_to_remaining_total(self):
        task = lab.load_tasks()[0]
        limits = []
        def fake_trial(image, output, selected, arm, repeat, effort="high", budget=None, credential=None):
            if arm == "baseline":
                return {"status": "completed", "passed": False, "verification": {"valid": True,
                    "returncode": 1, "outcome": {"tests_run": 10, "failures": 2, "errors": 0, "skipped": 0}}}
            if arm == "oracle":
                return {"passed": True}
            limits.append(budget)
            return {"status": "completed", "passed": True, "reported_cost_usd": 0.6 if len(limits) == 1 else 0.3}
        args = self.args(budget_usd=1, trial_budget_usd=5, arms=["claude-sonnet"], repeat=2,
                         task=None, image="image", seed=0, effort="high", suite="smoke")
        with tempfile.TemporaryDirectory() as directory, contextlib.redirect_stdout(io.StringIO()), \
             patch("lab.load_tasks", return_value=[task]), patch("lab.image_id", return_value="image-id"), \
             patch("lab.new_output", return_value=Path(directory)), patch("lab.preflight"), \
             patch("lab.provenance", return_value={}), patch("lab.report"), \
             patch("lab.resolve_credential", return_value={"type": "api", "key": "synthetic-test-key"}), \
             patch("lab.trial", side_effect=fake_trial):
            lab.run(args)
            ledger = json.loads((Path(directory) / "budget.json").read_text())
        self.assertAlmostEqual(limits[0], 1)
        self.assertAlmostEqual(limits[1], 0.4)
        self.assertAlmostEqual(ledger["known_api_spend_usd"], 0.9)
        self.assertTrue(ledger["api_accounting_complete"])

    def test_nonfinite_zero_negative_budgets_rejected(self):
        for value in ("nan", "inf", "-inf", "0", "-1"):
            with self.subTest(value=value), self.assertRaises(argparse.ArgumentTypeError):
                lab.finite_positive(value)
        self.assertEqual(lab.finite_positive("0.01"), 0.01)

    def test_snapshot_never_follows_symlinks_or_fifo(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "link").symlink_to("/etc/passwd")
            with self.assertRaisesRegex(ValueError, "special files"):
                lab.snapshot(root)
            (root / "link").unlink()
            (root / "nested").symlink_to("/etc", target_is_directory=True)
            with self.assertRaisesRegex(ValueError, "symlinks"):
                lab.snapshot(root)
            (root / "nested").unlink()
            os.mkfifo(root / "pipe")
            with self.assertRaisesRegex(ValueError, "special files"):
                lab.snapshot(root)

    def test_snapshot_ignores_agent_git_hooks(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / ".git").mkdir()
            (root / ".git" / "malicious-hook").write_text("not executed")
            (root / "code.py").write_text("print(1)\n")
            self.assertEqual(lab.snapshot(root), {"code.py": b"print(1)\n"})

    def test_sandbox_has_no_host_home_socket_or_verifier(self):
        with patch("lab.os.getuid", return_value=1000), patch("lab.os.getgid", return_value=1000):
            args = lab.sandbox_args("test-container")
        for expected in ("--read-only", "--cap-drop", "ALL", "no-new-privileges", "none"):
            self.assertIn(expected, args)
        self.assertNotIn("--privileged", args)
        self.assertFalse(any("docker.sock" in arg for arg in args))
        self.assertFalse(any("/verifier" in arg for arg in args))

    def test_report_counts_failures_and_unknown_cost_not_zero(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for index, result in enumerate([
                {"task": "t", "repeat": 1, "arm": "neoism-sol", "passed": False,
                 "reported_cost_usd": None, "duration_seconds": 180},
                {"task": "t", "repeat": 1, "arm": "claude-sonnet", "passed": True,
                 "reported_cost_usd": 0.5, "duration_seconds": 10},
            ]):
                folder = root / str(index)
                folder.mkdir()
                lab.write_json(folder / "result.json", result)
            results = lab.report(root)
            text = (root / "report.md").read_text()
            self.assertEqual(len(results), 2)
            self.assertIn("neoism-sol | 0/1 | 0.0% | unknown", text)
            self.assertIn("claude-sonnet | 1/1 | 100.0% | $0.5000", text)
            self.assertIn("claude-sonnet only 1", text)

    def test_tasks_have_consistent_complete_contracts(self):
        tasks = lab.load_tasks()
        self.assertEqual(len(tasks), 3)
        for task in tasks:
            self.assertEqual(task["id"], task["path"].name)
            self.assertTrue(lab.snapshot(task["path"] / "solution"))
        with self.assertRaises(ValueError):
            lab.load_tasks(["nonexistent"])

    def test_repository_suite_is_separate_and_verifiers_do_not_import_candidates(self):
        import ast
        repository = lab.load_tasks(suite="repository")
        self.assertEqual(len(repository), 10)
        self.assertEqual(len(lab.load_tasks()), 3)
        for task in repository:
            self.assertEqual(task["timeout_seconds"], 600)
            source = (task["path"] / "verifier" / "verify.py").read_text()
            modules = {node.module for node in ast.walk(ast.parse(source)) if isinstance(node, ast.ImportFrom)}
            self.assertIn("candidate_client", modules)
            self.assertTrue(modules <= {"candidate_client", "decimal", "collections", "itertools", "functools", "math"})

    def test_acceptance_scripts_do_not_import_candidate_modules(self):
        import ast
        forbidden = {"movements", "config_merge", "note_text"}
        for task in lab.load_tasks():
            source = (task["path"] / "verifier" / "verify.py").read_text()
            imports = {node.module for node in ast.walk(ast.parse(source)) if isinstance(node, ast.ImportFrom)}
            self.assertFalse(imports & forbidden)
            self.assertIn("candidate_client", imports)

    @patch.dict(os.environ, {"OPENAI_API_KEY": "fake-api-key"}, clear=True)
    def test_credentials_redacted(self):
        self.assertEqual(lab.redact("header fake-api-key"), "header [REDACTED]")

    def test_export_validation_before_host_writes(self):
        encoded = base64.b64encode(b"code\n").decode()
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "candidate"
            lab.restore_candidate({"src/code.py": encoded}, target)
            self.assertEqual((target / "src/code.py").read_bytes(), b"code\n")
            for index, files in enumerate(({"../escape": encoded}, {"/absolute": encoded},
                                            {".git/config": encoded}, {"a": encoded, "a/b": encoded},
                                            {"bad": "not base64!"})):
                destination = Path(directory) / str(index)
                with self.subTest(files=files), self.assertRaises(ValueError):
                    lab.restore_candidate(files, destination)
                self.assertFalse(destination.exists())

    def test_calibration_rejects_timeouts_import_errors_and_no_assertions(self):
        baseline = {"status": "completed", "passed": False, "verification": {
            "valid": True, "returncode": 1,
            "outcome": {"tests_run": 10, "failures": 2, "errors": 0, "skipped": 0}}}
        self.assertTrue(lab.calibrated_negative(baseline))
        for invalid in (
            dict(baseline, status="verifier_error"),
            dict(baseline, verification={"valid": False}),
            dict(baseline, verification={"valid": True, "outcome": {"failures": 0}}),
            dict(baseline, verification={"valid": True, "returncode": 1,
                 "outcome": {"tests_run": 10, "failures": 2, "errors": 1, "skipped": 0}}),
        ):
            self.assertFalse(lab.calibrated_negative(invalid))

    @patch("lab.subprocess.run")
    def test_cleanup_must_confirm_absence_and_retry(self, run):
        run.return_value = Mock(returncode=1, stdout="")
        with self.assertRaisesRegex(RuntimeError, "Cannot confirm cleanup"):
            lab.cleanup_container("owned-container")
        self.assertEqual(run.call_count, 4)
        run.reset_mock()
        run.return_value = Mock(returncode=0, stdout="")
        lab.cleanup_container("owned-container")
        self.assertEqual(run.call_count, 2)

    def test_streaming_redaction_handles_every_chunk_boundary_and_json_escaping(self):
        secret = 'synthetic-"token\\with-unicode-\u00e9'
        raw = ("before:" + secret + ":after").encode()
        escaped = json.dumps({"secret": secret}).encode()
        for payload in (raw, escaped):
            for split in range(len(payload) + 1):
                redactor = lab.StreamRedactor([secret])
                result = redactor.feed(payload[:split]) + redactor.feed(payload[split:]) + redactor.feed(b"", final=True)
                self.assertNotIn(secret.encode(), result)
                self.assertNotIn(json.dumps(secret)[1:-1].encode(), result)
                self.assertIn(b"[REDACTED]", result)

    @patch("lab.container")
    def test_subscription_is_token_metered_not_free_api_spend(self, execute):
        def mock_execution(image, folder, label, *args, **kwargs):
            if label == "agent":
                private_input = json.loads(kwargs["prompt"])
                self.assertEqual(private_input["credential"]["refresh"], "")
                self.assertNotIn("credential", kwargs)
                events = [{"type": "lab_result", "status": "completed", "reported_cost_usd": 0,
                           "accounting_complete": True, "usage_metered": True,
                           "tokens": {"input": 20, "output": 10}},
                          {"type": "lab_workspace", "files": {"code.py": base64.b64encode(b"pass\n").decode()}}]
            else:
                self.assertNotIn("secrets", kwargs)
                events = [{"type": "verification_result", "tests_run": 10,
                           "failures": 0, "errors": 0, "skipped": 0}]
            (folder / (label + ".jsonl")).write_text("\n".join(json.dumps(event) for event in events))
            return {"returncode": 0, "timed_out": False, "duration_seconds": 1}
        execute.side_effect = mock_execution
        credential = {"type": "oauth", "access": "synthetic-access", "refresh": "", "expires": 9999999999999}
        with tempfile.TemporaryDirectory() as directory, contextlib.redirect_stdout(io.StringIO()):
            output = Path(directory)
            result = lab.trial("image", output, lab.load_tasks()[0], "neoism-sol", 1, budget=1, credential=credential)
            lab.report(output)
            text = (output / "report.md").read_text()
        self.assertTrue(result["passed"])
        self.assertIsNone(result["reported_cost_usd"])
        self.assertEqual(result["estimated_usage_cost_usd"], 0)
        self.assertIn("subscription (not USD-priced)", text)
        self.assertNotIn("$0.0000", text)

    @patch("lab.container")
    def test_partial_accounting_never_becomes_complete_cost(self, execute):
        def mock_execution(image, folder, label, *args, **kwargs):
            if label == "agent":
                events = [{"type": "lab_result", "status": "timeout", "reported_cost_usd": 0.2,
                           "accounting_complete": False, "model_mismatch": False},
                          {"type": "lab_workspace", "files": {"code.py": base64.b64encode(b"pass\n").decode()}}]
            else:
                events = [{"type": "verification_result", "tests_run": 10,
                           "failures": 0, "errors": 0, "skipped": 0}]
            (folder / (label + ".jsonl")).write_text("\n".join(json.dumps(event) for event in events))
            return {"returncode": 0, "timed_out": False, "duration_seconds": 1}
        execute.side_effect = mock_execution
        with tempfile.TemporaryDirectory() as directory, contextlib.redirect_stdout(io.StringIO()):
            result = lab.trial("image", Path(directory), lab.load_tasks()[0], "neoism-sol", 1, budget=1,
                               credential={"type": "api", "key": "synthetic-test-key"})
        self.assertIsNone(result["reported_cost_usd"])
        self.assertEqual(result["partial_reported_cost_usd"], 0.2)
        self.assertFalse(result["passed"])

    @patch("lab.container")
    def test_self_test_arms_never_run_agents(self, execute):
        def execute_verifier(image, folder, label, *args, **kwargs):
            (folder / "verifier.jsonl").write_text(json.dumps({"type": "verification_result",
                "tests_run": 10, "failures": 0, "errors": 0, "skipped": 0}) + "\n")
            return {"returncode": 0, "timed_out": False, "duration_seconds": 0.1}
        execute.side_effect = execute_verifier
        task = lab.load_tasks()[0]
        with tempfile.TemporaryDirectory() as directory, contextlib.redirect_stdout(io.StringIO()):
            lab.trial("image-id", Path(directory), task, "oracle", 1)
        arguments = execute.call_args
        self.assertEqual(arguments.args[3], ["python3", "/opt/eval/verify.py"])
        self.assertNotIn("credential", arguments.kwargs)
        self.assertNotIn("network", arguments.kwargs)
        self.assertTrue(all(mount[2] for mount in arguments.kwargs["mounts"]))


class AdapterTests(unittest.TestCase):
    def assistant(self, cost=0.2, model="gpt-6.1-sol", **extra):
        return {"info": dict(role="assistant", cost=cost, modelId=model,
                             tokens={"input": 10, "output": 5, "reasoning": 2,
                                     "cache": {"read": 8, "write": 3}}, **extra),
                "parts": [{"type": "step-finish", "cost": 999}]}

    def test_usage_counts_all_sessions_not_step_finish_twice(self):
        summary = worker.usage({"root": [self.assistant()], "child": [self.assistant(cost=0.3)]})
        self.assertEqual(summary["reported_cost_usd"], 0.5)
        self.assertEqual(summary["tokens"]["input"], 20)
        self.assertEqual(summary["tokens"]["cache_read"], 16)
        self.assertEqual(summary["assistant_messages"], 2)

    def test_unknown_or_nonfinite_cost_is_unknown_not_free(self):
        for value in (None, float("nan"), float("inf"), -1):
            with self.subTest(cost=value):
                summary = worker.usage({"s": [self.assistant(cost=value)]})
                self.assertIsNone(summary["reported_cost_usd"])
        self.assertIsNone(worker.usage({})["reported_cost_usd"])

    def test_message_pagination_follows_actual_v2_cursor_shape(self):
        with patch("worker.request", side_effect=[
            {"items": [{"info": {"id": "a"}}], "cursor": {"next": "a"}},
            {"items": [{"info": {"id": "b"}}], "cursor": {}},
        ]) as request:
            result = worker.messages("session")
        self.assertEqual(len(result), 2)
        self.assertIn("cursor=a", request.call_args.args[1])

    def test_repeated_pagination_cursor_rejected(self):
        with patch("worker.request", return_value={"items": [], "cursor": {"next": "same"}}):
            with self.assertRaisesRegex(RuntimeError, "Repeated"):
                worker.messages("s")

    def test_collects_nested_subagents_once(self):
        def response(method, path, *args):
            children = {"/sessions/root/children": [{"id": "child"}],
                        "/sessions/child/children": [{"id": "root"}]}
            return {"items": children[path], "cursor": {}}
        with patch("worker.messages", return_value=[self.assistant()]), patch("worker.request", side_effect=response):
            result = worker.collect_sessions("root")
        self.assertEqual(set(result), {"root", "child"})

    def test_private_trial_input_and_child_only_credential_injection(self):
        import time
        credential = {"type": "oauth", "access": "synthetic-oauth-access", "refresh": "",
                      "expires": int(time.time() * 1000) + 3600000, "accountId": "synthetic-account"}
        raw = json.dumps({"prompt": "exact task", "credential": credential})
        prompt, selected = worker.parse_trial_input(raw, "neoism", 180)
        self.assertEqual(prompt, "exact task")
        with patch.dict(os.environ, {}, clear=True):
            env = worker.provider_environment("openai", selected)
            self.assertNotIn("NEOISM_AGENT_AUTH_CONTENT", os.environ)
            self.assertNotIn("OPENAI_API_KEY", env)
            self.assertEqual(json.loads(env["NEOISM_AGENT_AUTH_CONTENT"]), {"openai": credential})
            claude = worker.provider_environment("anthropic", {"type": "api", "key": "synthetic-anthropic"})
            self.assertEqual(claude["ANTHROPIC_API_KEY"], "synthetic-anthropic")
            self.assertNotIn("ANTHROPIC_API_KEY", os.environ)
        for invalid in (dict(credential, refresh="must-never-be-exported"), dict(credential, expires=0)):
            with self.assertRaises(ValueError):
                worker.parse_trial_input(json.dumps({"prompt": "task", "credential": invalid}), "neoism", 180)
        with self.assertRaises(ValueError):
            worker.parse_trial_input(raw, "claude", 180)

    def test_permissions_auto_allow_only_this_isolated_family(self):
        replies = []
        def response(method, path, body=None, **kwargs):
            if method == "GET":
                return ([{"id": "ours", "sessionId": "child", "permission": "external_directory", "patterns": ["/"]},
                         {"id": "other", "sessionId": "unrelated"}] if path.endswith("permissions") else [])
            replies.append((path, body))
        with patch("worker.request", side_effect=response), patch("worker.emit"):
            worker.service_interactions({"root", "child"})
        self.assertEqual(replies, [("/interactions/permissions/ours/reply", {"reply": "once"})])

    def test_native_completion_waits_for_family_not_idle_parent(self):
        polls = []
        runtimes = [
            {"execution": {"finished": False, "activeSegments": {}}, "runningBackgroundTasks": [{"sessionId": "child"}]},
            {"execution": {"finished": False, "activeSegments": {"child": 1}}, "runningBackgroundTasks": [{"sessionId": "child"}]},
            {"execution": {"finished": True, "activeSegments": {}}, "runningBackgroundTasks": []},
        ]
        def response(method, path, body=None, **kwargs):
            if path.startswith("/sessions?"):
                return {"id": "root"}
            if path.startswith("/interactions/"):
                return []
            if path.endswith("/runtime"):
                polls.append(path)
                return runtimes[len(polls) - 1]
            if path.endswith("/wait"):
                raise AssertionError("An idle parent is not authoritative family completion")
        def collect(root, sessions):
            sessions.update({"root": [self.assistant(time={"completed": 1})],
                             "child": [self.assistant(time={"completed": 1})]})
            return sessions
        with patch("worker.start_server", return_value=Mock()), patch("worker.request", side_effect=response), \
             patch("worker.collect_sessions", side_effect=collect), patch("worker.time.sleep"), patch("worker.emit") as emit:
            worker.run_neoism(argparse.Namespace(model="gpt-6.1-sol", effort="high", budget=1, timeout=600), "task")
        result = emit.call_args.args[0]
        self.assertEqual(len(polls), 3)
        self.assertEqual(result["status"], "completed")
        self.assertTrue(result["accounting_complete"])
        self.assertEqual(result["assistant_messages"], 2)

    def test_accounting_failure_cancels_family_and_marks_partial(self):
        import time
        aborted_sessions = set()
        def response(method, path, body=None, **kwargs):
            if path.startswith("/sessions?"):
                return {"id": "root"}
            if path.endswith("/abort"):
                aborted_sessions.add(path.split("/")[2])
        def broken_collection(root, sessions):
            sessions.update({"root": [self.assistant(model="unexpected-model")],
                             "child": [self.assistant()]})
            raise RuntimeError("accounting endpoint failed")
        with patch("worker.start_server", return_value=Mock()), patch("worker.request", side_effect=response), \
             patch("worker.collect_sessions", side_effect=broken_collection), patch("worker.emit") as emit:
            start = time.monotonic()
            worker.run_neoism(argparse.Namespace(model="gpt-6.1-sol", effort="high", budget=1, timeout=30), "task")
            self.assertLess(time.monotonic() - start, 1.5)
        result = emit.call_args.args[0]
        self.assertEqual(aborted_sessions, {"root", "child"})
        self.assertFalse(result["accounting_complete"])
        self.assertTrue(result["model_mismatch"])
        self.assertEqual(result["reported_cost_usd"], 0.4)

    def test_preflight_does_not_submit_prompt(self):
        process = Mock()
        calls = []
        def response(method, path, body=None, **kwargs):
            calls.append((method, path))
            if method == "POST":
                return {"id": "empty-session"}
            if path.startswith("/interactions/"):
                return []
            if path.endswith("/runtime"):
                return {"runningBackgroundTasks": [], "execution": None}
            return {"items": [], "cursor": {}}
        with patch("worker.subprocess.check_output", return_value="version"), \
             patch("worker.start_server", return_value=process), \
             patch("worker.request", side_effect=response), patch("worker.emit") as emit:
            worker.preflight()
        self.assertFalse(any("prompt" in path for _, path in calls))
        self.assertEqual(emit.call_args.args[0]["paid_requests"], 0)
        process.terminate.assert_called_once()


if __name__ == "__main__":
    unittest.main()
