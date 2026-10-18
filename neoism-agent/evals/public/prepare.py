#!/usr/bin/env python3
"""Prepare a pinned public benchmark. This command has no model-run operation."""

import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import importlib.metadata
import json
import os
from pathlib import Path
import random
import shutil
import subprocess
import sys
import tomllib
import urllib.request
import re
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parent
CACHE = ROOT / ".cache"
CHECKOUT = CACHE / "swe-bench-pro"
PROTOCOL = json.loads((ROOT / "protocol.json").read_text())


def output_json(path, value):
    path = Path(path).absolute()
    if any(part.is_symlink() for part in (path, *path.parents)):
        raise ValueError("Refusing symlink output path")
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(mode="w", dir=path.parent, prefix=".json-", delete=False) as stream:
        temporary = Path(stream.name)
        stream.write(json.dumps(value, indent=2, sort_keys=True) + "\n")
    try:
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def digest(path):
    value = hashlib.sha256()
    with path.open("rb") as source:
        while data := source.read(1024 * 1024):
            value.update(data)
    return value.hexdigest()


def command(args, timeout=120):
    if args[0] == "docker":
        # Registry metadata is anonymous even when invoked outside lab.py.
        with tempfile.TemporaryDirectory(prefix="public-metadata-") as home:
            from lab import scrub_environment
            return subprocess.check_output(args, text=True, timeout=timeout,
                                           env=scrub_environment(home)).strip()
    return subprocess.check_output(args, text=True, timeout=timeout).strip()


def fetch_dataset():
    CACHE.mkdir(exist_ok=True)
    if not CHECKOUT.exists():
        subprocess.run(["git", "clone", "--filter=blob:none", "--sparse", PROTOCOL["dataset"]["repository"], str(CHECKOUT)], check=True)
    if Path(command(["git", "-C", str(CHECKOUT), "rev-parse", "--show-toplevel"])) != CHECKOUT:
        raise ValueError("Dataset cache is not its own Git checkout")
    subprocess.run(["git", "-C", str(CHECKOUT), "fetch", "--depth", "1", "origin", PROTOCOL["dataset"]["commit"]], check=True)
    subprocess.run(["git", "-C", str(CHECKOUT), "checkout", "--detach", PROTOCOL["dataset"]["commit"]], check=True)
    subprocess.run(["git", "-C", str(CHECKOUT), "sparse-checkout", "set", "v2"], check=True)
    return verify_dataset()


def verify_dataset(checkout=CHECKOUT):
    if command(["git", "-C", str(checkout), "rev-parse", "HEAD"]) != PROTOCOL["dataset"]["commit"]:
        raise ValueError("Benchmark checkout differs from the pinned commit")
    if command(["git", "-C", str(checkout), "status", "--porcelain", "--untracked-files=all"]):
        raise ValueError("Benchmark checkout has local changes")
    base = checkout / "v2"
    verified = 0
    for line in (base / "SHA256SUMS").read_text().splitlines():
        expected, name = line.split(maxsplit=1)
        name = name.removeprefix("*")
        path = base / name
        if not path.resolve().is_relative_to(base.resolve()) or path.is_symlink() or not path.is_file():
            raise ValueError("Unsafe or missing upstream checksum target")
        regular_file(path)
        if digest(path) != expected:
            raise ValueError("Upstream checksum mismatch: " + name)
        verified += 1
    identifiers = [line.strip() for line in (base / "hard51_ids.txt").read_text().splitlines() if line.strip()]
    if len(identifiers) != 51 or len(set(identifiers)) != 51:
        raise ValueError("The official HARD-51 list must contain exactly 51 unique tasks")
    records = []
    for identifier in identifiers:
        path = base / "tasks" / identifier
        if path.name != identifier or not path.resolve().is_relative_to((base / "tasks").resolve()):
            raise ValueError("Unsafe task ID")
        config = tomllib.loads((path / "task.toml").read_text())
        if config["agent"]["timeout_sec"] != 3000 or config["agent"]["network_mode"] != "no-network":
            raise ValueError("Unexpected official agent policy")
        if config["verifier"]["network_mode"] != "public":
            raise ValueError("Unexpected official verifier policy")
        records.append({"id": identifier, "image_tag": config["environment"]["docker_image"],
                        "task_toml_sha256": digest(path / "task.toml"),
                        "instruction_sha256": digest(path / "instruction.md"),
                        "resources": {key: config["environment"].get(key) for key in ("cpus", "memory_mb", "storage_mb")}})
    lock = {"dataset": PROTOCOL["dataset"], "checksums_sha256": digest(base / "SHA256SUMS"),
            "subset_sha256": digest(base / "hard51_ids.txt"), "verified_source_files": verified, "tasks": records}
    output_json(ROOT / "hard51.lock.json", lock)
    return lock


def resolve_image(task):
    reference = task.get("locked_image", task["image_tag"])
    raw = json.loads(command(["docker", "manifest", "inspect", "--verbose", reference], timeout=120))
    entries = raw if isinstance(raw, list) else [raw]
    candidates = [entry for entry in entries if entry.get("Descriptor", {}).get("platform", {}).get("architecture") == "amd64"
                  and entry.get("Descriptor", {}).get("platform", {}).get("os") == "linux"]
    if not candidates and len(entries) == 1:
        candidates = entries
    if len(candidates) != 1:
        raise ValueError("No unambiguous Linux amd64 image for " + task["id"])
    selected = candidates[0]
    sha = selected["Descriptor"]["digest"]
    if not sha.startswith("sha256:") or len(sha) != 71:
        raise ValueError("Invalid registry manifest digest")
    repository = task["image_tag"].rsplit(":", 1)[0]
    manifest = selected.get("SchemaV2Manifest") or selected.get("OCIManifest")
    if not isinstance(manifest, dict) or not manifest.get("layers"):
        raise ValueError("Registry response omitted image layers; refusing an unknown download size")
    layers = manifest["layers"]
    return {"task": task["id"], "tag": task["image_tag"], "image": repository + "@" + sha,
            "digest": sha, "layers": [{"digest": layer["digest"], "size": layer["size"]} for layer in layers]}


def resolve_images(execute=False):
    if not execute:
        raise ValueError("Image metadata is a network operation; use images --execute explicitly")
    lock = verify_dataset()
    tasks = lock["tasks"]
    previous = None
    if (ROOT / "images.lock.json").exists():
        previous = validate_images(tasks)
        by_task = {item["task"]: item for item in previous["images"]}
        tasks = [dict(task, locked_image=by_task[task["id"]]["image"]) for task in tasks]
    with ThreadPoolExecutor(max_workers=3) as executor:
        images = list(executor.map(resolve_image, tasks))
    if previous and [item["image"] for item in images] != [by_task[task["id"]]["image"] for task in tasks]:
        raise ValueError("Metadata refresh must not repin existing digests")
    unique_layers = {layer["digest"]: layer["size"] for image in images for layer in image["layers"]}
    result = {"images": images, "unique_compressed_layer_bytes": sum(unique_layers.values()),
              "all_images_pulled": False, "note": "Registry metadata only; no model calls or image execution"}
    output_json(ROOT / "images.lock.json", result)
    return result


def regular_file(path):
    path = Path(path).absolute()
    if any(part.is_symlink() for part in (path, *path.parents)) or not path.is_file():
        raise ValueError("Expected a regular, non-symlink file: " + str(path))
    return path


def validate_assets():
    """Validate an EXISTING lock, never freeze, execute, or inspect host auth."""
    lock = json.loads(regular_file(ROOT / "assets.lock.json").read_text())
    if set(lock["agents"]) != set(PROTOCOL["harnesses"]):
        raise ValueError("Asset harness set differs from protocol")
    for name, asset in lock["agents"].items():
        if asset["version"] != PROTOCOL["harnesses"][name]["version"]:
            raise ValueError("Asset version differs from protocol")
        if digest(regular_file(asset["binary_path"])) != asset["binary_sha256"]:
            raise ValueError("Frozen binary changed: " + name)
    bundle = lock["runtime_bundle"]
    if digest(regular_file(bundle["path"])) != bundle["sha256"]:
        raise ValueError("Frozen runtime bundle changed")
    if (lock["agents"]["neoism"]["runtime_bundle"] != bundle["path"] or
            bundle["analyzed_binary_sha256"] != lock["agents"]["neoism"]["binary_sha256"]):
        raise ValueError("Runtime bundle binary binding differs")
    from assets import validate_archive
    validate_archive(bundle["path"])
    with tarfile.open(bundle["path"]) as archive:
        members = archive.getmembers()
        if {member.name for member in members} != set(bundle["files"]):
            raise ValueError("Bundle inventory differs")
        for member in members:
            expected = bundle["files"][member.name]
            stream = archive.extractfile(member)
            if (stream is None or member.size != expected["size"] or
                    hashlib.sha256(stream.read()).hexdigest() != expected["sha256"]):
                raise ValueError("Bundle member checksum differs: " + member.name)
    return lock


def validate_catalog():
    lock = json.loads(regular_file(ROOT / "model-catalog.lock.json").read_text())
    path = regular_file(CACHE / "models.json")
    if digest(path) != lock["sha256"]:
        raise ValueError("Frozen model catalog changed")
    model = json.loads(path.read_text())["anthropic"]["models"][PROTOCOL["model"]["id"]]
    if model != lock["model"]:
        raise ValueError("Catalog model metadata differs from lock")
    return lock


def source_lock_hashes():
    paths = [ROOT / "pyproject.toml", ROOT / "uv.lock"]
    paths.extend((ROOT / "opencode-v2").glob("*lock*.json"))
    paths.append(ROOT / "opencode-v2/package.json")
    return {str(path.relative_to(ROOT)): digest(regular_file(path)) for path in sorted(paths)}


def source_hashes():
    # Only executable sources, not caches, generated docs or job output.
    paths = list(ROOT.glob("*.py"))
    for directory in (ROOT / "neoism_harbor", ROOT / "tests"):
        paths.extend(directory.rglob("*.py"))
    return {str(path.relative_to(ROOT)): digest(regular_file(path)) for path in sorted(paths)}


def validate_images(tasks):
    lock = json.loads(regular_file(ROOT / "images.lock.json").read_text())
    images = lock["images"]
    if len(images) != len(tasks) or {item["task"] for item in images} != {task["id"] for task in tasks}:
        raise ValueError("Image lock must contain exactly the selected tasks")
    task_map = {task["id"]: task for task in tasks}
    layers = {}
    complete = True
    for item in images:
        task = task_map[item["task"]]
        if (item["tag"] != task["image_tag"] or not re.fullmatch(r"sha256:[0-9a-f]{64}", item["digest"])
                or item["image"] != task["image_tag"].rsplit(":", 1)[0] + "@" + item["digest"]):
            raise ValueError("Invalid pinned image reference")
        if not item.get("layers"):
            complete = False  # [] means UNKNOWN, never a zero-byte image.
        for layer in item.get("layers", []):
            size = layer.get("size")
            if (not re.fullmatch(r"sha256:[0-9a-f]{64}", layer["digest"]) or
                    type(size) is not int or size < 0):
                raise ValueError("Invalid image layer metadata")
            if layer["digest"] in layers and layers[layer["digest"]] != size:
                raise ValueError("Conflicting layer sizes")
            layers[layer["digest"]] = size
    return dict(lock, size_metadata_complete=complete,
                unique_compressed_layer_bytes=sum(layers.values()) if complete else None)


def materialize_subset(lock):
    destination = CACHE / "hard51"
    if any(part.is_symlink() for part in (destination, *destination.parents)):
        raise ValueError("Subset directory is a symlink")
    destination.mkdir(exist_ok=True)
    expected = {task["id"] for task in lock["tasks"]}
    if any(path.name not in expected or path.is_symlink() or not path.is_dir() for path in destination.iterdir()):
        raise ValueError("Subset cache contains unexpected entries; do not mix task sets")
    image_lock = validate_images(lock["tasks"])
    images = {image["task"]: image["image"] for image in image_lock["images"]}
    if set(images) != expected:
        raise ValueError("Resolve all 51 image digests before materializing tasks")
    for task in lock["tasks"]:
        source, target = CHECKOUT / "v2" / "tasks" / task["id"], destination / task["id"]
        original_toml = (source / "task.toml").read_text()
        old = 'docker_image = "' + task["image_tag"] + '"'
        new = 'docker_image = "' + images[task["id"]] + '"'
        if original_toml.count(old) != 1:
            raise ValueError("Cannot pin the official image reference unambiguously")
        prepared_toml = original_toml.replace(old, new)
        source_entries = {path.relative_to(source) for path in source.rglob("*")}
        if any(path.is_symlink() for path in source.rglob("*")):
            raise ValueError("Upstream task contains symlink")
        if target.exists():
            if {path.relative_to(target) for path in target.rglob("*")} != source_entries:
                raise ValueError("Unexpected or missing derived task files")
            if any(path.is_symlink() for path in target.rglob("*")):
                raise ValueError("Derived task contains symlink")
            for path in source.rglob("*"):
                relative = path.relative_to(source)
                if path.is_file() and relative != Path("task.toml") and (not (target / relative).is_file()
                                       or digest(path) != digest(target / relative)):
                    raise ValueError("Materialized subset was changed")
            if (target / "task.toml").read_text() not in (original_toml, prepared_toml):
                raise ValueError("Materialized task policy was changed")
        else:
            shutil.copytree(source, target)
        (target / "task.toml").write_text(prepared_toml)
    return destination


def validate_plan(path=None):
    """Read-only runner API; local import avoids prepare/lab initialization cycles."""
    from lab import verify_plan
    return verify_plan(path)


def make_plan(seed=0):
    lock = verify_dataset()
    subset = materialize_subset(lock)
    tasks = list(lock["tasks"])
    assets = validate_assets()
    catalog = CACHE / "models.json"
    catalog_lock = validate_catalog()
    random.Random(seed).shuffle(tasks)
    harnesses = list(PROTOCOL["harnesses"])
    slots = []
    # One-task jobs keep upstream patch replay unambiguous for each harness.
    # Rotating the harness order balances first/second/third position 17 times.
    for index, task in enumerate(tasks):
        order = harnesses[index % 3:] + harnesses[:index % 3]
        for harness in order:
            slot_id = f"{len(slots) + 1:03d}-{harness}"
            job_name = "hard51-" + slot_id
            job_dir = ROOT / "runs" / "hard51" / job_name
            artifact = assets["agents"][harness]
            agent_kwargs = {key: artifact[key] for key in ("binary_path", "binary_sha256", "runtime_bundle") if key in artifact}
            agent_kwargs.update(version=PROTOCOL["harnesses"][harness]["version"], catalog_path=str(catalog),
                                catalog_sha256=catalog_lock["sha256"])
            if "runtime_bundle" in artifact:
                agent_kwargs["runtime_bundle_sha256"] = assets["runtime_bundle"]["sha256"]
            config = {
                "job_name": job_name, "jobs_dir": str(ROOT / "runs" / "hard51"),
                "n_attempts": 1, "n_concurrent_trials": 1, "retry": {"max_retries": 0},
                "environment": {"type": "docker"},
                "agents": [{"import_path": PROTOCOL["harnesses"][harness]["class"],
                            "model_name": "anthropic/" + PROTOCOL["model"]["id"],
                            "extra_allowed_hosts": ["api.anthropic.com"],
                            "kwargs": agent_kwargs}],
                "tasks": [{"path": str(subset / task["id"])}],
            }
            from harbor.models.job.config import JobConfig
            JobConfig.model_validate(config)
            config_path = CACHE / "job-configs" / (slot_id + ".json")
            output_json(config_path, config)
            regrade = {
                "job_name": job_name + "-regrade", "jobs_dir": str(ROOT / "runs" / "hard51"),
                "n_attempts": 1, "n_concurrent_trials": 1, "retry": {"max_retries": 0},
                "environment": {"type": "docker"},
                "agents": [{"import_path": "neoism_harbor.replay:PatchReplayAgent", "model_name": "replay",
                            "kwargs": {"source_job": str(job_dir)}}],
                "tasks": [{"path": str(subset / task["id"])}],
            }
            JobConfig.model_validate(regrade)
            regrade_path = CACHE / "regrade-configs" / (slot_id + ".json")
            output_json(regrade_path, regrade)
            slots.append({"slot": slot_id, "task": task["id"], "harness": harness, "attempt": 1,
                          "job_config": str(config_path.relative_to(ROOT)),
                          "regrade_config": str(regrade_path.relative_to(ROOT)),
                           "job_config_sha256": digest(config_path),
                           "regrade_config_sha256": digest(regrade_path),
                           "run_allowed": False})
    if len(slots) != 153 or len({(slot["task"], slot["harness"], slot["attempt"]) for slot in slots}) != 153:
        raise ValueError("Invalid 153-trial plan")
    plan = {"schema_version": 2,
            "assets_lock_sha256": digest(ROOT / "assets.lock.json"),
            "images_lock_sha256": digest(ROOT / "images.lock.json"),
            "catalog_lock_sha256": digest(ROOT / "model-catalog.lock.json"),
            "catalog_sha256": catalog_lock["sha256"], "source_hashes": source_hashes(),
            "source_lock_hashes": source_lock_hashes(),
            "credential_resolver_sha256": digest(regular_file(ROOT.parent / "saved_auth.py")),
            "upstream_replay_sha256": digest(CHECKOUT / "v2/tooling/patch_replay.py"),
            "image_sizes": {key: validate_images(lock["tasks"])[key] for key in
                            ("size_metadata_complete", "unique_compressed_layer_bytes")},
            "protocol_sha256": digest(ROOT / "protocol.json"), "dataset_lock_sha256": digest(ROOT / "hard51.lock.json"),
            "seed": seed, "trials": slots, "paid_execution_enabled": False,
            "model": PROTOCOL["model"], "tasks": 51, "harnesses": 3, "attempts": 1,
            "note": "Setup plan only. New budget authorization, wire audits, network probes and calibration are required."}
    output_json(ROOT / "plan.json", plan)
    return plan


def freeze_catalog(execute=False):
    if (ROOT / "model-catalog.lock.json").exists():
        return validate_catalog()
    if not execute:
        raise ValueError("Catalog missing; explicitly use catalog --execute to fetch public metadata")
    req = urllib.request.Request("https://models.dev/api.json", headers={"User-Agent": "neoism-public-eval-setup"})
    with urllib.request.urlopen(req, timeout=45) as response:
        data = response.read(16 * 1024 * 1024 + 1)
    if len(data) > 16 * 1024 * 1024:
        raise ValueError("Model catalog exceeds setup limit")
    document = json.loads(data)
    model = document.get("anthropic", {}).get("models", {}).get(PROTOCOL["model"]["id"])
    if model is None:
        raise ValueError("Pinned model is absent from the public catalog; manual verified metadata is required")
    path = CACHE / "models.json"
    path.write_bytes(data)
    result = {"url": req.full_url, "sha256": digest(path), "model": model,
              "note": "Public metadata snapshot, not proof of account/model access"}
    output_json(ROOT / "model-catalog.lock.json", result)
    return result


def probe_environment(execute=False):
    """Compatibility API: explicit lab operation, never part of planning."""
    from lab import run_lab
    return run_lab("network-probe", execute=execute)


def doctor():
    versions = {}
    for package in ("harbor", "modal"):
        try:
            versions[package] = importlib.metadata.version(package)
        except importlib.metadata.PackageNotFoundError:
            versions[package] = None
    result = {"setup_only": True, "paid_execution_enabled": False, "versions": versions,
              "free_disk_gib": round(shutil.disk_usage(ROOT).free / 1024**3, 2),
              "dataset_fetched": CHECKOUT.exists(), "new_budget_authorization": False,
              "reasoning_wire_audit": "required", "full_51_oracle_nop_calibration": "not_run",
              "pristine_regrade": "required", "auth_values_read": False}
    result["docker_egress_kernel_support"] = "not_probed; explicit lab network-probe required"
    output_json(CACHE / "doctor.json", result)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=["fetch", "verify", "images", "plan", "catalog", "doctor"])
    parser.add_argument("--seed", type=int, default=0)
    parser.add_argument("--execute", action="store_true", help="Explicitly permit fetch or registry metadata; never model execution")
    args = parser.parse_args()
    if args.operation == "fetch" and not args.execute:
        parser.error("fetch requires --execute (network)")
    functions = {"fetch": fetch_dataset, "verify": verify_dataset, "images": lambda: resolve_images(args.execute),
                 "plan": lambda: make_plan(args.seed), "catalog": lambda: freeze_catalog(args.execute), "doctor": doctor}
    result = functions[args.operation]()
    if args.operation == "plan":
        print(json.dumps({"tasks": 51, "harnesses": 3, "attempts": 1, "trials": len(result["trials"]), "paid_execution_enabled": False}))
    elif args.operation == "images":
        print(json.dumps({"pinned_images": len(result["images"]), "unique_compressed_layer_bytes": result["unique_compressed_layer_bytes"]}))
    elif args.operation in ("fetch", "verify"):
        print(json.dumps({"tasks": len(result["tasks"]), "verified_source_files": result["verified_source_files"], "commit": result["dataset"]["commit"]}))
    elif args.operation == "catalog":
        print(json.dumps({"catalog_sha256": result["sha256"], "model": PROTOCOL["model"]["id"]}))
    else:
        print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
