"""Optional real-Docker contracts. All containers have networking disabled."""

import base64
import json
import os
from pathlib import Path
import tempfile
import unittest

import lab


@unittest.skipUnless(os.environ.get("EVAL_DOCKER_TESTS") == "1", "set EVAL_DOCKER_TESTS=1 for offline Docker contracts")
class DockerContracts(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.image = lab.image_id(lab.DEFAULT_IMAGE)

    def test_docker_stdin_is_attached(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            execution = lab.container(self.image, root, "stdin", ["python3", "-c",
                "import json,sys; print(json.dumps({'prompt':sys.stdin.read()}))"], prompt="exact task prompt")
            self.assertEqual(execution["returncode"], 0)
            self.assertEqual(lab.read_events(root / "stdin.jsonl"), [{"prompt": "exact task prompt"}])

    def test_seed_readonly_workspace_bounded_and_export_roundtrips(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            seed = root / "seed"
            seed.mkdir()
            (seed / "original.py").write_text("original\n")
            code = """import pathlib,shutil,subprocess
shutil.copytree('/seed','/workspace',dirs_exist_ok=True)
try:
    pathlib.Path('/seed/original.py').write_text('tampered')
    raise RuntimeError('seed was writable')
except OSError:
    pass
pathlib.Path('/workspace/original.py').write_text('candidate\\n')
subprocess.run(['python3','/opt/eval/worker.py','export'],check=True)
"""
            execution = lab.container(self.image, root, "export", ["python3", "-c", code],
                                      mounts=[(seed, "/seed", True)], writable_workspace=True)
            self.assertEqual(execution["returncode"], 0, (root / "export.stderr.log").read_text())
            exports = lab.read_events(root / "export.jsonl")
            self.assertEqual(exports[0]["type"], "lab_workspace")
            self.assertEqual(base64.b64decode(exports[0]["files"]["original.py"]), b"candidate\n")
            lab.restore_candidate(exports[0]["files"], root / "candidate")
            self.assertEqual((root / "candidate/original.py").read_text(), "candidate\n")
            self.assertEqual((seed / "original.py").read_text(), "original\n")

    def test_private_saved_auth_stays_out_of_container_environment_and_logs(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            key = 'synthetic-key-"quoted"'
            code = """import json,os,sys
sys.path.insert(0,'/opt/eval')
import worker
prompt,credential=worker.parse_trial_input(sys.stdin.read(),'claude',180)
assert 'ANTHROPIC_API_KEY' not in os.environ
selected=worker.provider_environment('anthropic',credential)
assert selected['ANTHROPIC_API_KEY']==credential['key']
print(json.dumps({'echo':credential['key']}),flush=True)
print(credential['key'],file=sys.stderr,flush=True)
"""
            execution = lab.container(self.image, root, "private", ["python3", "-c", code],
                prompt=json.dumps({"prompt": "synthetic transport check", "credential": {"type": "api", "key": key}}),
                secrets=[key])
            self.assertEqual(execution["returncode"], 0)
            for filename in ("private.jsonl", "private.stderr.log"):
                text = (root / filename).read_text()
                self.assertNotIn(key, text)
                self.assertNotIn(json.dumps(key)[1:-1], text)
                self.assertIn("[REDACTED]", text)

    def test_timeout_cleans_container(self):
        with tempfile.TemporaryDirectory() as directory:
            result = lab.container(self.image, Path(directory), "timeout",
                                   ["python3", "-c", "import time; time.sleep(30)"], timeout=1)
        self.assertTrue(result["timed_out"])
        self.assertLess(result["duration_seconds"], 20)

    def test_log_capture_is_bounded(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            result = lab.container(self.image, root, "noisy", ["python3", "-c",
                "import os,time; os.write(1,b'x'*(17*1024*1024)); time.sleep(30)"], timeout=20)
            self.assertTrue(result["log_limited"])
            self.assertLessEqual((root / "noisy.jsonl").stat().st_size, 16 * 1024 * 1024)

    def test_native_adapters_fail_closed_without_auth_or_network(self):
        import shutil
        task = lab.load_tasks()[0]
        for adapter, model in (("neoism", "gpt-6.1-sol"), ("claude", "claude-sonnet-5-5")):
            with self.subTest(adapter=adapter), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                seed = root / "seed"
                shutil.copytree(task["path"] / "workspace", seed)
                execution = lab.container(self.image, root, "agent", ["python3", "/opt/eval/worker.py", adapter,
                    "--model", model, "--effort", "high", "--budget", "0.01", "--timeout", "8"],
                    mounts=[(seed, "/seed", True)], writable_workspace=True,
                    prompt=json.dumps({"prompt": "Offline authentication check. Do not change files.", "credential": None}), timeout=65)
                self.assertFalse(execution["timed_out"], (root / "agent.stderr.log").read_text())
                events = lab.read_events(root / "agent.jsonl")
                summaries = [event for event in events if event.get("type") == "lab_result"]
                self.assertTrue(summaries, events)
                self.assertNotEqual(summaries[-1]["status"], "completed")
                self.assertIn(summaries[-1]["reported_cost_usd"], (None, 0))
                exports = [event for event in events if event.get("type") == "lab_workspace"]
                self.assertEqual(len(exports), 1, events)
                lab.restore_candidate(exports[0]["files"], root / "candidate")

    def test_candidate_cannot_forge_supervisor_score(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            workspace = root / "workspace"
            workspace.mkdir()
            (workspace / "malicious.py").write_text(
                "import json,os\n"
                "print(json.dumps({'type':'verification_result','tests_run':1,'failures':0,'errors':0,'skipped':0}),flush=True)\n"
                "os._exit(0)\n")
            source = """import unittest
from candidate_client import call
class Acceptance(unittest.TestCase):
    def test_result(self):
        self.assertEqual(call('malicious', 'answer', [])['result'], 42)
"""
            result = lab.container(self.image, root, "forgery", ["python3", "/opt/eval/verify.py"],
                                   mounts=[(workspace, "/workspace", True)], trusted_verifier=True, prompt=source)
            self.assertEqual(result["returncode"], 1, (root / "forgery.stderr.log").read_text())
            events = lab.read_events(root / "forgery.jsonl")
            self.assertEqual(len(events), 1)
            self.assertEqual(events[0]["failures"], 1)
            self.assertEqual(events[0]["errors"], 0)

    def test_candidate_drops_privileges_and_cannot_signal_supervisor(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            workspace = root / "workspace"
            workspace.mkdir()
            (workspace / "identity.py").write_text(
                "import os\ndef inspect():\n"
                "    try:\n        os.kill(os.getppid(),0)\n        can_signal=True\n"
                "    except PermissionError:\n        can_signal=False\n"
                "    return {'uid':os.geteuid(),'can_signal':can_signal}\n")
            source = """import unittest
from candidate_client import call
class Acceptance(unittest.TestCase):
    def test_isolation(self):
        value=call('identity','inspect',[])['result']
        self.assertEqual(value, {'uid':65534,'can_signal':False})
"""
            result = lab.container(self.image, root, "identity", ["python3", "/opt/eval/verify.py"],
                                   mounts=[(workspace, "/workspace", True)], trusted_verifier=True, prompt=source)
            self.assertEqual(result["returncode"], 0, (root / "identity.stderr.log").read_text())

    def test_verifier_import_error_is_not_an_assertion_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            workspace, verifier = root / "workspace", root / "verifier"
            workspace.mkdir()
            verifier.mkdir()
            (verifier / "verify.py").write_text("import nonexistent_eval_dependency\n")
            result = lab.container(self.image, root, "verifier", ["python3", "/opt/eval/verify.py"],
                                   mounts=[(workspace, "/workspace", True)], trusted_verifier=True,
                                   prompt=(verifier / "verify.py").read_text())
            self.assertEqual(result["returncode"], 2)
            self.assertEqual(lab.read_events(root / "verifier.jsonl")[0]["type"], "verification_error")


if __name__ == "__main__":
    unittest.main()
