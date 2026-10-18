"""Mocked repository/network probe contracts; source only until the lab phase."""
import json
from pathlib import Path
import tempfile
import types
import unittest
from unittest.mock import AsyncMock

from neoism_harbor.probe import ProtocolProbe


def clean_report():
    empty = lambda: {"code": 0, "output": ""}
    return {"network": {"https://api.anthropic.com/v1/models": {"reachable": True, "status": 401},
                        "https://api.github.com": {"reachable": False},
                        "https://pypi.org/simple/": {"reachable": False},
                        "https://registry.npmjs.org": {"reachable": False}},
            "repository": "/app", "head_commits": {"code": 0, "output": "1"},
            "all_commits": {"code": 0, "output": "1"}, "reflog": empty(), "stash": empty(),
            "fix_object": {"code": 128, "output": ""}, "fsck": empty(),
            "hidden_directories": [], "hidden_test_files": [], "historical_heads": [], "non_sample_hooks": []}


class ProbeContracts(unittest.IsolatedAsyncioTestCase):
    async def execute_mock(self, report, root):
        task = root / ("instance_fixture-" + "a" * 40 + "-vfixture")
        (task / "tests").mkdir(parents=True)
        (task / "tests/test_patch.patch").write_text("--- /dev/null\n+++ b/hidden-test.py\n")
        logs = root / "trial/agent"
        logs.mkdir(parents=True)
        (logs.parent / "config.json").write_text(json.dumps({"task": {"path": str(task)}}))
        probe = ProtocolProbe(logs_dir=logs)
        environment = types.SimpleNamespace(
            network_policy=types.SimpleNamespace(network_mode=types.SimpleNamespace(value="allowlist"), allowed_hosts=["api.anthropic.com"]),
            capabilities=types.SimpleNamespace(dynamic_network_policy=True, network_allowlist=True),
            exec=AsyncMock(return_value=types.SimpleNamespace(return_code=0, stdout=json.dumps(report))))
        context = types.SimpleNamespace()
        await probe.run("not given to a model", environment, context)
        return environment, context

    async def test_clean_report_checks_hidden_files_and_dangling_objects(self):
        with tempfile.TemporaryDirectory() as temporary:
            environment, context = await self.execute_mock(clean_report(), Path(temporary))
            command = environment.exec.call_args.args[0]
            self.assertIn("fsck", command)
            self.assertIn("hidden-test.py", command)
            self.assertEqual(context.metadata["inference_calls"], 0)
            self.assertFalse(context.metadata["credentials_read"])

    async def test_leaks_and_forbidden_network_are_rejected(self):
        mutations = (
            ("hidden_directories", ["/tests"]), ("hidden_test_files", ["hidden-test.py"]),
            ("historical_heads", ["FETCH_HEAD"]), ("non_sample_hooks", ["post-checkout"]),
            ("fix_object", {"code": 0, "output": "commit"}),
            ("fsck", {"code": 0, "output": "unreachable commit abc"}),
            ("all_commits", {"code": 0, "output": "2"}))
        for key, value in mutations:
            with self.subTest(key=key), tempfile.TemporaryDirectory() as temporary:
                report = clean_report()
                report[key] = value
                with self.assertRaises(RuntimeError):
                    await self.execute_mock(report, Path(temporary))
        with tempfile.TemporaryDirectory() as temporary:
            report = clean_report()
            report["network"]["https://api.github.com"]["reachable"] = True
            with self.assertRaises(RuntimeError):
                await self.execute_mock(report, Path(temporary))


if __name__ == "__main__":
    unittest.main()
