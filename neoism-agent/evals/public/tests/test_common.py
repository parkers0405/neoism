"""Admission/accounting/recovery contracts; execute only during the lab phase."""

from datetime import datetime, timedelta, timezone
import hashlib
import json
import os
from pathlib import Path
import tempfile
import types
import unittest
from unittest.mock import AsyncMock, patch

from neoism_harbor import common


def save(path, value):
    path.write_text(json.dumps(value))
    return hashlib.sha256(path.read_bytes()).hexdigest()


class AdmissionContracts(unittest.TestCase):
    def fixture(self, root):
        save(root / "protocol.json", {"dataset": {"commit": "pinned"}})
        config_sha = save(root / "config.json", {"job": "one"})
        plan_sha = save(root / "plan.json", {"trials": [{"slot": "001-neoism", "job_config": "config.json"}]})
        approval = {"approved": True, "budget_control_verified": True, "maximum_api_cost_usd": 100,
                    "maximum_trial_api_cost_usd": 10, "dataset_commit": "pinned", "model": common.MODEL,
                    "attempts": 1, "plan_sha256": plan_sha,
                    "expires_at": (datetime.now(timezone.utc) + timedelta(hours=1)).isoformat()}
        approval_path = root / "approval.json"
        approval_sha = save(approval_path, approval)
        admission_path = root / "admission.json"
        save(admission_path, {"slot": "001-neoism", "plan_sha256": plan_sha,
                              "source_config_sha256": config_sha, "approval_sha256": approval_sha,
                              "maximum_api_cost_usd": 10})
        return approval_path, admission_path

    def test_default_disabled_before_reading_approval(self):
        with patch.dict(os.environ, {}, clear=True), patch.object(Path, "read_text") as read:
            with self.assertRaises(RuntimeError):
                common.require_authorization("must-not-read.json", "must-not-read-admission.json")
            read.assert_not_called()

    def test_admission_is_single_use(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            approval, admission = self.fixture(root)
            with patch.object(common, "ROOT", root), patch.dict(os.environ, {"NEOISM_PUBLIC_EVAL_ALLOW_INFERENCE": "1"}):
                document, reservation = common.require_authorization(approval, admission)
                self.assertEqual(reservation["maximum_api_cost_usd"], 10)
                self.assertEqual(document["model"], common.MODEL)
                with self.assertRaises(FileExistsError):
                    common.require_authorization(approval, admission)

    def test_expiry_wrong_plan_and_excess_reservation_rejected(self):
        for mutation in ("expiry", "plan", "reservation"):
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                approval, admission = self.fixture(root)
                if mutation == "expiry":
                    document = json.loads(approval.read_text())
                    document["expires_at"] = "2000-01-01T00:00:00Z"
                    save(approval, document)
                elif mutation == "plan":
                    save(root / "plan.json", {"trials": []})
                else:
                    document = json.loads(admission.read_text())
                    document["maximum_api_cost_usd"] = 11
                    save(admission, document)
                with patch.object(common, "ROOT", root), patch.dict(os.environ, {"NEOISM_PUBLIC_EVAL_ALLOW_INFERENCE": "1"}):
                    with self.assertRaises(RuntimeError):
                        common.require_authorization(approval, admission)
                self.assertFalse(Path(str(admission) + ".consumed").exists())


class AccountingContracts(unittest.TestCase):
    def test_raw_anthropic_usage_includes_both_cache_categories(self):
        context = types.SimpleNamespace()
        common.populate_context(context, {"usage": {"input_tokens": 5, "cache_read_input_tokens": 11,
            "cache_creation_input_tokens": 7, "output_tokens": 3}, "total_cost_usd": .1,
            "estimated_cost_usd": .02, "accounting_complete": True})
        self.assertEqual(context.n_input_tokens, 23)
        self.assertEqual(context.n_cache_tokens, 11)
        self.assertEqual(context.n_output_tokens, 3)
        self.assertEqual(context.cost_usd, .1)
        self.assertEqual(context.metadata["estimated_cost_usd"], .02)

    def test_estimated_price_is_not_native_invoice(self):
        context = types.SimpleNamespace()
        common.populate_context(context, {"tokens": {"input_total": 12, "cache_read": 3,
            "cache_write": 4, "output_total": 2}, "estimated_cost_usd": .03})
        self.assertIsNone(context.cost_usd)
        self.assertEqual(context.n_input_tokens, 12)
        self.assertFalse(context.metadata["accounting_complete"])

    def test_nonfinite_negative_and_boolean_counters_fail(self):
        for value in (-1, True, float("nan"), 1.5):
            with self.subTest(value=value), self.assertRaises(RuntimeError):
                common.populate_context(types.SimpleNamespace(), {"tokens": {"input_total": value}})

    def test_capture_uses_private_index_and_disables_external_diff(self):
        self.assertIn("GIT_INDEX_FILE", common._CAPTURE)
        self.assertIn("--no-ext-diff --no-textconv", common._CAPTURE)
        self.assertNotIn(" reset ", common._CAPTURE)


class RecoveryContracts(unittest.IsolatedAsyncioTestCase):
    async def test_authorization_failure_never_reads_credential_or_executes(self):
        class Fixture(common.LockedAgentBase):
            @staticmethod
            def name():
                return "offline-admission-fixture"

        with tempfile.TemporaryDirectory() as temporary:
            adapter = Fixture(logs_dir=Path(temporary))
            environment = types.SimpleNamespace(exec=AsyncMock())
            with patch.dict(os.environ, {}, clear=True), patch.object(common, "selected_api_credential") as credentials:
                with self.assertRaises(RuntimeError):
                    await adapter.run("unused", environment, types.SimpleNamespace())
                credentials.assert_not_called()
                environment.exec.assert_not_called()


if __name__ == "__main__":
    unittest.main()
