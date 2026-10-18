"""Freeze native evaluation assets; setup only, with no credentials or model calls.

Only --version is executed. Docker images are never pulled implicitly. The lock is
reused and verified, so subsequent preparation does not consult mutable CLI paths.
"""
from __future__ import annotations

import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import platform
import re
import shutil
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parent
CACHE = ROOT / ".cache" / "artifacts"
LOCK = ROOT / "assets.lock.json"
SOURCE_IMAGE = "neoism-eval-lab:local"
SPECS = {
    "neoism": (Path.home() / ".local/bin/neoism-agent", "0.7.128", None),
    "claude-code": (Path.home() / ".local/share/mise/installs/claude/latest/claude", "2.1.292", "a967e7b1d8b4e47ee421d5433027880347952b0c0857abf880e2c942a4ec93b3"),
    "opencode-v2": (ROOT / ".cache/opencode-v2/node_modules/@opencode/cli-linux-x64/bin/opencode", "2.0.32", "489b51816a77809e841310e80c88e834c28c19ad1639e12ba8a19f08b47ab607"),
}
WRAPPER = b'''#!/bin/sh
# Scope the private glibc to this process; children retain the task runtime.
exec /opt/neoism-public-eval/lib/ld-linux-x86-64.so.2 --library-path /opt/neoism-public-eval/lib /opt/neoism-public-eval/bin/neoism-agent.real "$@"
'''


def sha256(path):
    digest = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def run(args, **kwargs):
    kwargs.setdefault("stderr", subprocess.PIPE)
    return subprocess.run([str(a) for a in args], check=True, stdout=subprocess.PIPE,
                          timeout=180, **kwargs).stdout


def version_number(output):
    match = re.search(r"(?<![\d.])(\d+\.\d+\.\d+)(?![\d.])", output)
    if not match:
        raise ValueError("No native version in output: " + output[:200])
    return match.group(1)


def freeze_binary(source, version, expected_sha, cache):
    """Copy first, validate and execute the immutable copy, never the source."""
    source, cache = Path(source), Path(cache)
    cache.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=str(cache)) as temp:
        staged = Path(temp) / "binary"
        shutil.copyfile(str(source), str(staged))
        with staged.open("rb") as stream:
            if stream.read(4) != b"\x7fELF":
                raise ValueError("Native ELF required: " + str(source))
        digest = sha256(staged)
        if expected_sha and digest != expected_sha:
            raise ValueError("Native binary SHA mismatch: " + str(source))
        staged.chmod(0o555)
        output = run([staged, "--version"], env={"PATH": "/usr/bin:/bin", "HOME": temp,
                      "XDG_CONFIG_HOME": temp, "XDG_CACHE_HOME": temp}).decode().strip()
        if version_number(output) != version:
            raise ValueError("Native version mismatch: " + output)
        target = cache / (digest + ".elf")
        if target.exists():
            if sha256(target) != digest:
                raise ValueError("Corrupt frozen asset: " + str(target))
        else:
            os.replace(str(staged), str(target))
    return {"binary_path": str(target.resolve()), "binary_sha256": digest,
            "version": version, "version_output": output,
            "source_path": str(source), "host_platform": {
                "system": platform.system(), "machine": platform.machine(),
                "libc": list(platform.libc_ver())}}


def validate_archive(archive):
    """Reject traversal, links, special files, duplicate or unexpected entries.

    Bundles deliberately contain regular files only, eliminating link-chain and
    extraction-order escapes as well as accidentally archived credentials/ELFs.
    """
    seen = set()
    with tarfile.open(str(archive), "r:*") as tar:
        for member in tar.getmembers():
            name = member.name
            path = PurePosixPath(name)
            if (path.is_absolute() or ".." in path.parts or "\\" in name
                    or str(path) != name or name in seen or not member.isfile()):
                raise ValueError("Unsafe runtime archive entry: " + name)
            if not (name == "bin/neoism-agent" or
                    (len(path.parts) == 2 and path.parts[0] == "lib"
                     and (".so" in path.name or path.name.startswith("ld-linux")))):
                raise ValueError("Unexpected runtime archive entry: " + name)
            if member.mode & 0o7000:
                raise ValueError("Unsafe runtime archive permissions: " + name)
            seen.add(name)
    if "bin/neoism-agent" not in seen or "lib/ld-linux-x86-64.so.2" not in seen or "lib/libc.so.6" not in seen:
        raise ValueError("Incomplete private runtime archive")


def image_info(image):
    info = json.loads(run(["docker", "image", "inspect", image]))[0]
    if info["Os"] != "linux" or info["Architecture"] != "amd64":
        raise ValueError("Private runtime requires linux/amd64")
    return {"image_id": info["Id"], "platform": "linux/amd64",
            "repo_digests": info.get("RepoDigests", [])}


def build_runtime(binary, cache, image=SOURCE_IMAGE):
    provenance = image_info(image)
    image_id = provenance["image_id"]
    mount = "type=bind,src=" + str(binary) + ",dst=/agent-input,readonly"
    closure = run(["docker", "run", "--rm", "--network", "none", "--read-only",
                   "--mount", mount, "--entrypoint", "/usr/bin/ldd", image_id,
                   "/agent-input"]).decode()
    if "not found" in closure or "=>" not in closure:
        raise ValueError("Source image ABI closure missing: " + closure)
    paths = sorted(set(re.findall(r"(?:=>\s+|^\s*)(/[^\s]+)", closure, re.M)))
    if not any(Path(p).name == "ld-linux-x86-64.so.2" for p in paths):
        raise ValueError("Source image loader missing: " + closure)
    # Dereference image symlinks inside the image; only explicit ldd paths enter tar.
    raw = run(["docker", "run", "--rm", "--network", "none", "--read-only",
               "--entrypoint", "/bin/tar", image_id, "-chf", "-", "--"] + paths)
    files = {"bin/neoism-agent": WRAPPER}
    origins = {}
    with tarfile.open(fileobj=io.BytesIO(raw)) as tar:
        members = tar.getmembers()
        if len(members) != len(paths):
            raise ValueError("Unexpected dependency tar contents")
        for member in members:
            source = "/" + member.name.lstrip("/")
            if source not in paths or not member.isfile():
                raise ValueError("Unexpected dependency: " + member.name)
            name = "lib/" + PurePosixPath(source).name
            if name in files:
                raise ValueError("Dependency basename collision: " + name)
            extracted = tar.extractfile(member)
            if extracted is None:
                raise ValueError("Unreadable dependency: " + member.name)
            files[name] = extracted.read()
            origins[name] = source
    cache = Path(cache)
    cache.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=str(cache)) as temp:
        archive = Path(temp) / "runtime.tar"
        with tarfile.open(str(archive), "w", format=tarfile.USTAR_FORMAT) as tar:
            for name, content in sorted(files.items()):
                member = tarfile.TarInfo(name)
                member.size, member.mode, member.mtime = len(content), 0o555, 0
                tar.addfile(member, io.BytesIO(content))
        validate_archive(archive)
        digest = sha256(archive)
        target = cache / (digest + ".tar")
        if target.exists() and sha256(target) != digest:
            raise ValueError("Corrupt runtime cache")
        os.replace(str(archive), str(target))
        target.chmod(0o444)
    return {"path": str(target.resolve()), "sha256": digest,
            "source_image": dict(provenance, requested_reference=image),
            "analyzed_binary_sha256": sha256(binary), "ldd_output": closure,
            "files": {name: {"sha256": hashlib.sha256(data).hexdigest(),
                              "size": len(data), "source_path": origins.get(name, "generated-wrapper")}
                      for name, data in sorted(files.items())}}


def check_compatibility(agents, bundle, images=("debian:bookworm-slim", "ubuntu:22.04")):
    """Check locally available images only; never pull or modify their OS."""
    results = []
    with tempfile.TemporaryDirectory(dir=str(CACHE)) as temp:
        root = Path(temp)
        validate_archive(bundle["path"])
        with tarfile.open(bundle["path"]) as tar:
            # Regular-file-only allowlist was validated above.
            tar.extractall(str(root), filter="data")
        shutil.copyfile(agents["neoism"]["binary_path"], str(root / "bin/neoism-agent.real"))
        (root / "bin/neoism-agent.real").chmod(0o555)
        mounts = ["--mount", "type=bind,src=" + str(root) + ",dst=/opt/neoism-public-eval,readonly"]
        commands = {"neoism": "/opt/neoism-public-eval/bin/neoism-agent"}
        for name in ("claude-code", "opencode-v2"):
            target = "/" + name
            commands[name] = target
            mounts += ["--mount", "type=bind,src=" + agents[name]["binary_path"] + ",dst=" + target + ",readonly"]
        for image in images:
            try:
                info = image_info(image)
            except subprocess.CalledProcessError:
                results.append({"image": image, "status": "unavailable_not_pulled"})
                continue
            # One fresh container checks all three native executables. No host home,
            # sockets, network, credentials, privileged mode, or global linker env.
            script = "\n".join("printf '\\nASSET:%s\\n' '" + name + "'; " + command +
                               " --version; printf 'EXIT:%s\\n' \"$?\"" for name, command in commands.items())
            output = run(["docker", "run", "--rm", "--network", "none", "--read-only",
                          "--tmpfs", "/tmp:rw,nosuid,nodev", "-e", "HOME=/tmp", "-e", "XDG_CACHE_HOME=/tmp/cache",
                          "-e", "XDG_CONFIG_HOME=/tmp/config"] + mounts +
                         ["--entrypoint", "/bin/sh", info["image_id"], "-c", script], stderr=subprocess.STDOUT).decode(errors="replace")
            checks = {}
            for name, body, code in re.findall(r"ASSET:([^\n]+)\n(.*?)EXIT:(\d+)", output, re.S):
                try:
                    matched = version_number(body) == agents[name]["version"]
                except ValueError:
                    matched = False
                checks[name] = {"return_code": int(code), "output": body.strip(),
                                "status": "passed" if code == "0" and matched else "failed"}
            if set(checks) != set(agents):
                raise ValueError("Incomplete container compatibility output: " + output)
            results.append(dict(info, image=image, checks=checks))
    return {"scope": "--version only; not model calls or task execution", "images": results}


def verify_lock(lock):
    for agent in lock["agents"].values():
        if sha256(agent["binary_path"]) != agent["binary_sha256"]:
            raise ValueError("Frozen binary changed")
    bundle = lock["runtime_bundle"]
    if sha256(bundle["path"]) != bundle["sha256"]:
        raise ValueError("Frozen runtime changed")
    validate_archive(bundle["path"])
    if lock["agents"]["neoism"]["runtime_bundle"] != bundle["path"]:
        raise ValueError("Runtime bundle path mismatch")
    return lock


def freeze_assets():
    """Return verified frozen assets; no mutable host path is used after first lock.

    Agent entries include audit metadata; merge binary_path/binary_sha256/version
    and (for Neoism) runtime_bundle into plan kwargs as supported by the adapter.
    Compatibility is untested until explicit container --version checks are made.
    """
    if LOCK.exists():
        return verify_lock(json.loads(LOCK.read_text()))
    if platform.system() != "Linux" or platform.machine() not in ("x86_64", "AMD64"):
        raise ValueError("Asset preparation requires Linux x86_64")
    agents = {name: freeze_binary(source, version, digest, CACHE)
              for name, (source, version, digest) in SPECS.items()}
    bundle = build_runtime(agents["neoism"]["binary_path"], CACHE)
    agents["neoism"]["runtime_bundle"] = bundle["path"]
    result = {"schema_version": 1, "agents": agents, "runtime_bundle": bundle,
              "compatibility": check_compatibility(agents, bundle)}
    # Atomic publication; failed preparation cannot leave a partial lock.
    with tempfile.NamedTemporaryFile(mode="w", dir=str(ROOT), prefix=".assets-", delete=False) as stream:
        temporary = Path(stream.name)
        json.dump(result, stream, indent=2, sort_keys=True)
        stream.write("\n")
    try:
        os.replace(str(temporary), str(LOCK))
    finally:
        temporary.unlink(missing_ok=True)
    return result


if __name__ == "__main__":
    print(json.dumps(freeze_assets(), indent=2, sort_keys=True))
