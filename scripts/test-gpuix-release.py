#!/usr/bin/env python3
"""Validate an extracted, installed release with private daemon/window state."""
import argparse
import os
from pathlib import Path
import subprocess
import tempfile


def main():
    repo = Path(__file__).resolve().parent.parent
    frontend = repo / "neoism-frontend/gpuix"
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", nargs="?", type=Path,
                        default=repo / "release/neoism-linux-x86_64.tar.gz")
    parser.add_argument("--without-window", action="store_true")
    args = parser.parse_args()

    def run(command, **kwargs):
        subprocess.run([str(arg) for arg in command], check=True, timeout=300, **kwargs)

    with tempfile.TemporaryDirectory(prefix="neoism-release-check-") as temporary:
        directory = Path(temporary)
        run(["tar", "-xzf", args.archive.resolve(), "-C", directory])
        payload = directory / "neoism-linux-x86_64"
        prefix = directory / "installed"
        run(["bash", payload / "install-bundle.sh", payload, prefix])
        binary = prefix / "bin/neoism"
        run([binary, "--version"], cwd=directory)
        run([binary, "--help"], cwd=directory)
        for resource in ["LICENSE", "NOTICE", "licenses/Geist-OFL.txt",
                         "licenses/ICU4X.txt", "licenses/Tantivy.txt", "licenses/Tantivy-AUTHORS.txt", "licenses/Turso.txt",
                         "licenses/GPUIX-native.txt", "licenses/GPUIX-solid.txt",
                         "web/agent-gui/index.html"]:
            assert (prefix / "bin" / resource).is_file(), resource
        env = {**os.environ,
               "NEOISM_GPUIX_TEST_BINARY": str(binary),
               "NEOISM_GPUIX_WORKSPACE_DAEMON": str(prefix / "bin/neoism-workspace-daemon")}
        bun = frontend / "node_modules/.bin/bun"
        for flags in [[], ["--without-xdg-runtime"]]:
            run([bun, "scripts/local-daemon-smoke.ts", *flags], cwd=frontend, env=env)
        if not args.without_window:
            run([bun, "scripts/native-local-startup-check.ts"], cwd=frontend, env=env)
            for script in ["native-splash-check.ts", "native-tab-workflow-check.ts"]:
                run([bun, "scripts/" + script, "--packaged"], cwd=frontend, env=env)
        print("Installed release checks passed.")


if __name__ == "__main__":
    main()
