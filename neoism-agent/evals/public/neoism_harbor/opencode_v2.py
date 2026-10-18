"""Private pinned OpenCode V2 native artifact; no global npm/CLI changes."""
import hashlib
import os
from pathlib import Path

from .common import LockedAgentBase

VERSION = "2.0.32"
NATIVE_SHA256 = "489b51816a77809e841310e80c88e834c28c19ad1639e12ba8a19f08b47ab607"
# Native linux-x64 binary from the integrity-locked @opencode/cli-linux-x64.
# Install the public/opencode-v2 lock into public/.cache/opencode-v2 with npm ci.
PRIVATE_PACKAGE = Path(__file__).resolve().parents[1] / ".cache/opencode-v2"


class OpenCodeV2Sonnet(LockedAgentBase):
    runtime_name = "opencode_v2"
    binary = "/opt/neoism-public-eval/bin/opencode-v2"

    def version(self):
        return VERSION

    @staticmethod
    def name():
        return "opencode-v2-sonnet-locked"

    def __init__(self, *args, binary_path=None, binary_sha256=None, **kwargs):
        self.native_binary_path = binary_path
        self.native_binary_sha256 = binary_sha256
        super().__init__(*args, **kwargs)

    async def runtime_binary(self):
        return self.binary

    async def install_cli(self, environment):
        source = Path(self.native_binary_path or os.environ.get(
            "NEOISM_PUBLIC_EVAL_OPENCODE_V2_BINARY",
            str(PRIVATE_PACKAGE / "node_modules/@opencode/cli-linux-x64/bin/opencode")))
        expected = self.native_binary_sha256 or os.environ.get("NEOISM_PUBLIC_EVAL_OPENCODE_V2_SHA256") or NATIVE_SHA256
        if expected != NATIVE_SHA256:
            raise ValueError("OpenCode V2 requires the integrity-locked linux-x64 2.0.32 artifact")
        with source.open("rb") as stream:
            if stream.read(4) != b"\x7fELF":
                raise ValueError("OpenCode V2 requires native ELF, not a host CLI launcher")
            stream.seek(0)
            digest = hashlib.file_digest(stream, "sha256").hexdigest()
        if digest != expected:
            raise ValueError("OpenCode V2 native artifact SHA256 mismatch")
        # Harbor 0.22.0 BaseEnvironment.exec/upload_file. This runs inside the
        # isolated evaluation environment, never in the host's global prefix.
        result = await self.exec_as_root(environment, "mkdir -p /opt/neoism-public-eval/bin /tmp/opencode-v2-version-home", timeout_sec=30)
        if result.return_code:
            raise RuntimeError("Cannot create private OpenCode V2 directory")
        await environment.upload_file(source_path=str(source), target_path=self.binary)
        command = ("chmod 755 " + self.binary + " && env -i PATH=/usr/bin:/bin "
                   "HOME=/tmp/opencode-v2-version-home " + self.binary + " --version")
        result = await self.exec_as_root(environment, command, timeout_sec=30)
        if result.return_code or (result.stdout or "").strip() != "opencode v2.0.32":
            raise RuntimeError("OpenCode V2 readiness failed: native 2.0.32 / compatible ABI required")
