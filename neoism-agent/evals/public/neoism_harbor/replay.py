"""Pristine replay adapted from pinned v2/tooling/patch_replay.py.

Keep upstream missing/empty-patch semantics, but require one source task/trial,
accept partial patches even after agent timeout, and never use fuzzy/3-way apply.
Application failures are infrastructure failures, not silently zero rewards.
"""
from __future__ import annotations

import hashlib
import json
from pathlib import Path

from harbor.agents.base import BaseAgent


def safe_file(path):
    path = Path(path).absolute()
    if any(item.is_symlink() for item in (path, *path.parents)) or not path.is_file():
        raise ValueError("Unsafe or missing replay file: " + str(path))
    return path


def source_patch(source_job, task_name, patch_name="model.patch"):
    """Return (patch or None, source trial); no success-result prerequisite."""
    if patch_name != "model.patch":
        raise ValueError("Only the captured model.patch may be replayed")
    job = Path(source_job).absolute()
    config = json.loads(safe_file(job / "config.json").read_text())
    tasks = config.get("tasks", [])
    if (config.get("datasets") or len(tasks) != 1 or
            Path(tasks[0]["path"]).name != task_name or config.get("n_attempts", 1) != 1):
        raise ValueError("Replay source must be a one-task, one-attempt job for this task")
    trials = []
    for entry in job.iterdir():
        if not entry.is_dir():
            continue
        if entry.is_symlink():
            raise ValueError("Symlink in source job")
        trial_config = entry / "config.json"
        if not trial_config.exists():
            continue
        trial = json.loads(safe_file(trial_config).read_text())
        if Path(trial["task"]["path"]).name != task_name:
            raise ValueError("Unexpected task in replay source")
        trials.append(entry)
    if len(trials) != 1:
        raise ValueError("Replay source must contain exactly one trial, including failed/partial trials")
    patch = trials[0] / "agent" / patch_name
    if any(item.is_symlink() for item in (patch, *patch.parents)):
        raise ValueError("Symlink patch path")
    return (safe_file(patch) if patch.exists() else None), trials[0]


class PatchReplayAgent(BaseAgent):
    def __init__(self, *args, source_job, patch_name="model.patch", **kwargs):
        super().__init__(*args, **kwargs)
        self.source_job = source_job
        self.patch_name = patch_name

    @staticmethod
    def name():
        return "patch-replay-agent"

    def version(self):
        return "2.0.0"

    async def setup(self, environment):
        pass

    async def run(self, instruction, environment, context):
        config = json.loads(safe_file(self.logs_dir.parent / "config.json").read_text())
        task = Path(config["task"]["path"]).name
        patch, trial = source_patch(self.source_job, task, self.patch_name)
        self.logs_dir.mkdir(parents=True, exist_ok=True)
        patch_bytes = patch.read_bytes() if patch else b""
        report = {"task": task, "source_trial": str(trial), "patch": str(patch) if patch else None,
                  "patch_sha256": hashlib.sha256(patch_bytes).hexdigest() if patch else None,
                  "empty_patch": not patch_bytes,
                  "inference_calls": 0, "credentials_read": False}
        try:
            if not report["empty_patch"]:
                snapshot = self.logs_dir / "replay-source.patch"
                snapshot.write_bytes(patch_bytes)
                await environment.upload_file(source_path=snapshot, target_path="/tmp/replay.patch")
                command = r'''set -eu
repo=$(git -C /app rev-parse --show-toplevel 2>/dev/null || git -C /testbed rev-parse --show-toplevel)
case "$repo" in /app|/testbed) ;; *) exit 91;; esac
cd "$repo"
git -c core.hooksPath=/dev/null -c core.fsmonitor=false apply --check --binary /tmp/replay.patch
git -c core.hooksPath=/dev/null -c core.fsmonitor=false apply --binary /tmp/replay.patch
'''
                result = await environment.exec(command=command, timeout_sec=300)
                report.update(apply_rc=result.return_code,
                              apply_out=((result.stdout or "") + (result.stderr or ""))[-6000:])
                if result.return_code:
                    raise RuntimeError("Pristine patch check/apply failed; this is not a false reward")
            report["status"] = "applied" if not report["empty_patch"] else "empty"
            context.metadata = {"replay": report}
        except Exception as error:
            report.update(status="infrastructure_error", error=str(error))
            raise
        finally:
            (self.logs_dir / "replay.json").write_text(json.dumps(report, indent=2) + "\n")
