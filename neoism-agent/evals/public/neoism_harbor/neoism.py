"""Frozen native Neoism installation; no network installer or system upgrade."""
import hashlib
import os
from pathlib import Path
import shlex

from .common import LockedAgentBase


class NeoismSonnet(LockedAgentBase):
    runtime_name = "neoism"
    binary = "/opt/neoism-public-eval/bin/neoism-agent"

    @staticmethod
    def name():
        return "neoism-sonnet-locked"

    def __init__(self, *args, binary_path=None, runtime_bundle=None,
                 binary_sha256=None, **kwargs):
        self.native_binary_path = binary_path
        self.native_runtime_bundle = runtime_bundle
        self.native_binary_sha256 = binary_sha256
        super().__init__(*args, binary_path=binary_path, runtime_bundle=runtime_bundle, **kwargs)

    async def runtime_binary(self):
        return self.binary

    async def install_cli(self, environment):
        source = self.native_binary_path or os.environ.get("NEOISM_PUBLIC_EVAL_NEOISM_BINARY")
        if not source or not Path(source).is_file():
            raise ValueError("Provide binary_path or NEOISM_PUBLIC_EVAL_NEOISM_BINARY (frozen 0.7.128 artifact)")
        with Path(source).open("rb") as stream:
            if stream.read(4) != b"\x7fELF":
                raise ValueError("Neoism artifact must be native ELF, not a host launcher")
            stream.seek(0)
            digest = hashlib.file_digest(stream, "sha256").hexdigest()
        expected = self.native_binary_sha256 or os.environ.get("NEOISM_PUBLIC_EVAL_NEOISM_SHA256")
        if expected and digest != expected:
            raise ValueError("Neoism artifact SHA256 mismatch")
        result = await self.exec_as_root(environment, command="mkdir -p /opt/neoism-public-eval/bin", timeout_sec=30)
        if result.return_code:
            raise RuntimeError("Cannot create private native binary directory")
        if self.native_runtime_bundle:
            # Validate the locked private runtime before extracting it in a task image.
            import json
            from assets import validate_archive
            from .common import ROOT
            bundle = json.loads((ROOT / "assets.lock.json").read_text())["runtime_bundle"]
            if Path(self.native_runtime_bundle).resolve() != Path(bundle["path"]).resolve():
                raise ValueError("Neoism runtime must be the frozen private bundle")
            with Path(self.native_runtime_bundle).open("rb") as stream:
                if hashlib.file_digest(stream, "sha256").hexdigest() != bundle["sha256"]:
                    raise ValueError("Neoism private runtime SHA256 mismatch")
            validate_archive(self.native_runtime_bundle)
            await environment.upload_file(source_path=self.native_runtime_bundle,
                                          target_path="/opt/neoism-public-eval/runtime-bundle.tar")
            result = await self.exec_as_root(environment, command="tar -xf /opt/neoism-public-eval/runtime-bundle.tar -C /opt/neoism-public-eval", timeout_sec=60)
            if result.return_code:
                raise RuntimeError("Cannot unpack parent-prepared private runtime bundle")
            target = self.binary + ".real"
        else:
            target = self.binary
        await environment.upload_file(source_path=source, target_path=target)
        result = await self.exec_as_root(environment, command="chmod 755 " + shlex.quote(target) + " && mkdir -p /tmp/neoism-version-home && env -i PATH=/usr/bin:/bin HOME=/tmp/neoism-version-home " + self.binary + " --version", timeout_sec=30)
        if result.return_code or (result.stdout or "").strip() != "neoism-agent 0.7.128":
            raise RuntimeError("Neoism readiness failed: frozen 0.7.128 required; check private loader/GLIBC ABI")
