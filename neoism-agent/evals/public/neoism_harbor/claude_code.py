"""Upload a frozen native Claude Code artifact; never run an unpinned bootstrap."""
import hashlib
import os
from pathlib import Path

from .common import LockedAgentBase


class ClaudeSonnet(LockedAgentBase):
    runtime_name = "claude_code"
    binary = "/opt/neoism-public-eval/bin/claude"

    @staticmethod
    def name():
        return "claude-sonnet-locked"

    def __init__(self, *args, binary_path=None, binary_sha256=None, **kwargs):
        self.native_binary_path = binary_path
        self.native_binary_sha256 = binary_sha256
        super().__init__(*args, binary_path=binary_path, **kwargs)

    async def runtime_binary(self):
        return self.binary

    async def install_cli(self, environment):
        source = self.native_binary_path or os.environ.get("NEOISM_PUBLIC_EVAL_CLAUDE_BINARY")
        expected = self.native_binary_sha256 or os.environ.get("NEOISM_PUBLIC_EVAL_CLAUDE_SHA256")
        if not source or not expected:
            raise ValueError("Frozen native Claude artifact requires binary_path and binary_sha256 (or NEOISM_PUBLIC_EVAL_CLAUDE_BINARY/SHA256)")
        artifact = Path(source)
        with artifact.open("rb") as stream:
            if stream.read(4) != b"\x7fELF":
                raise ValueError("Claude artifact must be native ELF, not a host launcher or bootstrap")
        with artifact.open("rb") as stream:
            if hashlib.file_digest(stream, "sha256").hexdigest() != expected:
                raise ValueError("Claude artifact SHA256 mismatch")
        result = await self.exec_as_root(environment, command="mkdir -p /opt/neoism-public-eval/bin", timeout_sec=30)
        if result.return_code:
            raise RuntimeError("Cannot create private native binary directory")
        await environment.upload_file(source_path=str(artifact), target_path=self.binary)
        result = await self.exec_as_root(environment, command="chmod 755 " + self.binary + " && mkdir -p /tmp/claude-version-home && env -i PATH=/usr/bin:/bin HOME=/tmp/claude-version-home " + self.binary + " --version", timeout_sec=30)
        if result.return_code or (result.stdout or "").strip() != "2.1.292 (Claude Code)":
            raise RuntimeError("Claude readiness failed: frozen native 2.1.292/compatible ABI required")
