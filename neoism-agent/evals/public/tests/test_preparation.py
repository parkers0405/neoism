"""Preparation/lab regression sources. Do not run as part of plan generation."""
import hashlib
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import prepare
import lab


class PreparationTests(unittest.TestCase):
    def test_missing_asset_lock_never_freezes_or_launches(self):
        with tempfile.TemporaryDirectory() as temporary, patch.object(prepare, "ROOT", Path(temporary)):
            with patch("subprocess.run", side_effect=AssertionError("execution forbidden")), patch("subprocess.check_output", side_effect=AssertionError("execution forbidden")):
                with self.assertRaises(ValueError):
                    prepare.validate_assets()

    def test_symlink_regular_file_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "file").write_text("fixture")
            (root / "link").symlink_to(root / "file")
            with self.assertRaises(ValueError):
                prepare.regular_file(root / "link")
            (root / "directory").symlink_to(root, target_is_directory=True)
            with self.assertRaises(ValueError):
                prepare.regular_file(root / "directory/file")

    def test_unknown_image_sizes_are_not_zero(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            digest = "sha256:" + "a" * 64
            image = {"task": "fixture", "tag": "example/image:fixture", "digest": digest,
                     "image": "example/image@" + digest, "layers": []}
            (root / "images.lock.json").write_text(json.dumps({"images": [image], "unique_compressed_layer_bytes": 0}))
            with patch.object(prepare, "ROOT", root):
                result = prepare.validate_images([{"id": "fixture", "image_tag": image["tag"]}])
            self.assertIsNone(result["unique_compressed_layer_bytes"])
            self.assertFalse(result["size_metadata_complete"])
            with self.assertRaises(ValueError):
                lab.pull_images(result, {}, 20, 100)

    def test_existing_catalog_is_not_refetched(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "model-catalog.lock.json").write_text("{}")
            with patch.object(prepare, "ROOT", root), patch.object(prepare, "validate_catalog", return_value={"fixture": True}) as validate:
                with patch("urllib.request.urlopen", side_effect=AssertionError("network forbidden")):
                    self.assertEqual(prepare.freeze_catalog(execute=True), {"fixture": True})
                    validate.assert_called_once()

    def test_lab_without_execute_is_a_description_only(self):
        with patch.object(lab, "verify_plan", side_effect=AssertionError("no execution")), patch("subprocess.run", side_effect=AssertionError("no execution")):
            for operation in lab.OPERATIONS:
                self.assertFalse(lab.run_lab(operation)["executed"])

    def test_scrub_does_not_forward_credentials_or_enablement(self):
        with tempfile.TemporaryDirectory() as temporary, patch.dict(os.environ, {
                "ANTHROPIC_API_KEY": "synthetic", "NEOISM_PUBLIC_EVAL_ALLOW_INFERENCE": "1",
                "OPENAI_API_KEY": "synthetic", "CLAUDE_CODE_OAUTH_TOKEN": "synthetic",
                "AWS_PROFILE": "synthetic", "HTTP_PROXY": "synthetic"}):
            env = lab.scrub_environment(Path(temporary))
            for name in ("ANTHROPIC_API_KEY", "NEOISM_PUBLIC_EVAL_ALLOW_INFERENCE", "OPENAI_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN", "AWS_PROFILE", "HTTP_PROXY"):
                self.assertNotIn(name, env)
            self.assertEqual(env["HOME"], temporary)

    def test_verifier_infrastructure_is_not_a_false_reward(self):
        with tempfile.TemporaryDirectory() as temporary:
            job = Path(temporary)
            trial = job / "trial"
            trial.mkdir()
            result = {"task_name": "fixture", "finished_at": "fixture", "exception_info": {"type": "infra"},
                      "verifier_result": {"rewards": {"reward": 0}}}
            (trial / "result.json").write_text(json.dumps(result))
            with self.assertRaises(RuntimeError):
                lab.trial_result(job, "fixture")
            result["exception_info"] = None
            result["verifier_result"] = None
            (trial / "result.json").write_text(json.dumps(result))
            with self.assertRaises(RuntimeError):
                lab.trial_result(job, "fixture")

    def test_offline_workers_require_network_namespaces(self):
        for worker in ("_unit", "_wire"):
            command = lab.isolated_worker_command(worker)
            self.assertEqual(command[:5], ["unshare", "--user", "--map-root-user", "--net", "--"])
            self.assertIn('ip link set lo up; exec "$@"', command)
            self.assertIn("--execute", command)

    def test_source_partial_patch_does_not_need_success_result(self):
        from neoism_harbor.replay import source_patch
        with tempfile.TemporaryDirectory() as temporary:
            job = Path(temporary)
            (job / "config.json").write_text(json.dumps({"tasks": [{"path": "/fixture"}], "n_attempts": 1}))
            trial = job / "trial"
            (trial / "agent").mkdir(parents=True)
            (trial / "config.json").write_text(json.dumps({"task": {"path": "/fixture"}}))
            captured = trial / "agent/model.patch"
            captured.write_text("partial fixture patch")
            self.assertEqual(source_patch(job, "fixture"), (captured, trial))
            (trial / "extra").mkdir()
            second = job / "second"
            second.mkdir()
            (second / "config.json").write_text((trial / "config.json").read_text())
            with self.assertRaises(ValueError):
                source_patch(job, "fixture")


if __name__ == "__main__":
    unittest.main()
