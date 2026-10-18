"""Fail-closed native Harbor admission, accounting, recovery and patch capture."""

from __future__ import annotations

from datetime import datetime, timezone
import hashlib
import json
import math
import os
from pathlib import Path
import shlex
import tempfile

from harbor.agents.installed.base import BaseInstalledAgent

ROOT = Path(__file__).resolve().parents[1]
MODEL = "claude-sonnet-5-5"
PROVIDER = "anthropic"
API_BASE_URL = "https://api.anthropic.com/v1"
TIMEOUT_SECONDS = 3000
REMOTE = "/opt/neoism-public-eval"


def read_protocol():
    return json.loads((ROOT / "protocol.json").read_text())


def file_digest(path):
    with Path(path).open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def positive_amount(value):
    return type(value) in (int, float) and math.isfinite(value) and value > 0


def require_authorization(path, admission_path=None):
    if os.environ.get("NEOISM_PUBLIC_EVAL_ALLOW_INFERENCE") != "1" or not path or not admission_path:
        raise RuntimeError("Inference requires explicit enablement, budget approval and a runner-issued admission")
    document = json.loads(Path(path).read_text())
    protocol = read_protocol()
    try:
        expires = datetime.fromisoformat(document["expires_at"].replace("Z", "+00:00"))
        valid_expiry = expires.tzinfo is not None and expires > datetime.now(timezone.utc)
    except (KeyError, TypeError, ValueError, AttributeError):
        valid_expiry = False
    if (document.get("approved") is not True or document.get("budget_control_verified") is not True
            or not positive_amount(document.get("maximum_api_cost_usd"))
            or not positive_amount(document.get("maximum_trial_api_cost_usd"))
            or document["maximum_trial_api_cost_usd"] > document["maximum_api_cost_usd"]
            or document.get("dataset_commit") != protocol["dataset"]["commit"]
            or document.get("model") != MODEL or type(document.get("attempts")) is not int or document["attempts"] != 1
            or document.get("plan_sha256") != file_digest(ROOT / "plan.json") or not valid_expiry):
        raise RuntimeError("Missing, expired or incompatible public-run authorization")
    admission_file = Path(admission_path)
    admission = json.loads(admission_file.read_text())
    if (admission.get("approval_sha256") != file_digest(path)
            or admission.get("plan_sha256") != document["plan_sha256"]
            or not positive_amount(admission.get("maximum_api_cost_usd"))
            or admission["maximum_api_cost_usd"] > document["maximum_trial_api_cost_usd"]):
        raise RuntimeError("Invalid public-run budget reservation")
    plan = json.loads((ROOT / "plan.json").read_text())
    slots = [slot for slot in plan["trials"] if slot["slot"] == admission.get("slot")]
    if len(slots) != 1 or file_digest(ROOT / slots[0]["job_config"]) != admission.get("source_config_sha256"):
        raise RuntimeError("Admission is not bound to an immutable planned slot")
    # Even a repeated Harbor invocation cannot consume the same admission twice.
    descriptor = os.open(str(admission_file) + ".consumed", os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    with os.fdopen(descriptor, "w") as marker:
        marker.write(datetime.now(timezone.utc).isoformat() + "\n")
        marker.flush()
        os.fsync(marker.fileno())
    return document, admission


def selected_api_credential(path=None, connection=None):
    # The resolver exports only the selected API key, never refresh/unrelated auth.
    import importlib.util
    source = ROOT.parent / "saved_auth.py"
    plan = json.loads((ROOT / "plan.json").read_text())
    if plan.get("credential_resolver_sha256") != file_digest(source):
        raise RuntimeError("Saved-auth resolver changed since planning")
    spec = importlib.util.spec_from_file_location("neoism_eval_saved_auth", source)
    if spec is None or spec.loader is None:
        raise RuntimeError("Saved-auth resolver cannot be loaded")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    document = module.load_document(path)
    return module.projection("anthropic", module.select_credential(document, "anthropic", connection), 0)


def populate_context(context, result):
    tokens = result.get("tokens") or result.get("usage") or {}

    def count(*names):
        for name in names:
            value = tokens.get(name)
            if value is not None:
                if type(value) is not int or value < 0:
                    raise RuntimeError("Invalid native token accounting")
                return value
        return 0

    cache_read = count("cache_read", "cache_read_input_tokens")
    cache_write = count("cache_write", "cache_creation_input_tokens")
    input_total = tokens.get("input_total")
    if input_total is None:
        input_total = count("input_uncached", "input_tokens", "input") + cache_read + cache_write
    if type(input_total) is not int or input_total < cache_read + cache_write:
        raise RuntimeError("Invalid total input accounting")
    context.n_input_tokens = input_total
    context.n_cache_tokens = cache_read
    context.n_output_tokens = count("output_total", "output_tokens", "output")
    # Native reported dollars and frozen-catalog estimates are different metrics.
    reported = result.get("reported_cost_usd", result.get("total_cost_usd"))
    if reported is not None and (type(reported) not in (int, float) or not math.isfinite(reported) or reported < 0):
        raise RuntimeError("Invalid native reported cost")
    context.cost_usd = reported
    context.metadata = {"same_model_audit": result, "fresh_sandbox_regrade_required": True,
                        "estimated_cost_usd": result.get("estimated_cost_usd"),
                        "accounting_complete": result.get("accounting_complete", False)}


# Use a separate index: preserve staged changes and capture tracked/untracked files.
# Fresh regrading trusts only this patch, never executable files in the solver sandbox.
_CAPTURE = r'''set -eu
repo=$(git -c core.hooksPath=/dev/null -c core.fsmonitor=false -C /app rev-parse --show-toplevel 2>/dev/null || git -c core.hooksPath=/dev/null -c core.fsmonitor=false -C /testbed rev-parse --show-toplevel 2>/dev/null)
case "$repo" in /app|/testbed) ;; *) exit 42 ;; esac
cd "$repo"
mkdir -p /logs/agent
index=$(mktemp /tmp/neoism-eval-index.XXXXXX)
rm -f "$index"
trap 'rm -f "$index"' EXIT
export GIT_INDEX_FILE="$index"
git -c core.hooksPath=/dev/null -c core.fsmonitor=false read-tree HEAD
git -c core.hooksPath=/dev/null -c core.fsmonitor=false add -A
git -c core.hooksPath=/dev/null -c core.fsmonitor=false diff --cached --binary --no-ext-diff --no-textconv HEAD > /logs/agent/model.patch.tmp
mv /logs/agent/model.patch.tmp /logs/agent/model.patch
'''
_REPOSITORY = "git -c core.hooksPath=/dev/null -c core.fsmonitor=false -C /app rev-parse --show-toplevel 2>/dev/null || git -c core.hooksPath=/dev/null -c core.fsmonitor=false -C /testbed rev-parse --show-toplevel 2>/dev/null"


class LockedAgentBase(BaseInstalledAgent):
    """Run the original harness; the supervisor never implements its agent loop."""

    runtime_name = ""
    MODEL_CONNECTION = None

    def __init__(self, *args, authorization_path=None, admission_path=None, auth_path=None,
                 connection_id=None, binary_path=None, runtime_bundle=None, catalog_path=None,
                 **kwargs):
        model_name = kwargs.pop("model_name", None) or "anthropic/" + MODEL
        if model_name not in (MODEL, "anthropic/" + MODEL):
            raise ValueError("This experiment is pinned to anthropic/" + MODEL)
        self.authorization_path = authorization_path
        self.admission_path = admission_path
        self.auth_path = auth_path
        self.connection_id = connection_id
        self.binary_path = binary_path
        self.runtime_bundle = runtime_bundle
        self.catalog_path = catalog_path
        super().__init__(*args, model_name="anthropic/" + MODEL, **kwargs)

    async def install(self, environment):
        await self.ensure_system_dependencies(environment, ("python3", "bash", "git"))
        result = await self.exec_as_root(environment, "mkdir -p " + REMOTE + "/bin")
        if result.return_code:
            raise RuntimeError("Cannot create runtime installation directory")
        for name in (self.runtime_name + ".py", "runtime_utils.py", "anthropic_gateway.py"):
            await environment.upload_file(ROOT / "neoism_harbor" / "runtime" / name, REMOTE + "/" + name)
        await self.install_cli(environment)

    async def install_cli(self, environment):
        raise NotImplementedError("Harness-specific installation is required")

    async def runtime_binary(self):
        return REMOTE + "/bin/" + self.runtime_name

    async def run(self, instruction, environment, context):
        approval, admission = require_authorization(self.authorization_path, self.admission_path)
        policy = environment.network_policy
        if getattr(policy.network_mode, "value", policy.network_mode) != "allowlist" or set(policy.allowed_hosts) != {"api.anthropic.com"}:
            raise RuntimeError("Agent phase must allow only api.anthropic.com")
        capabilities = environment.capabilities
        if not capabilities.dynamic_network_policy or not capabilities.network_allowlist:
            raise RuntimeError("The environment cannot enforce the locked network protocol")
        protocol = read_protocol()
        result = await environment.exec(_REPOSITORY, timeout_sec=30)
        directory = (result.stdout or "").strip()
        if result.return_code or directory not in ("/app", "/testbed"):
            raise RuntimeError("No official benchmark repository found")
        catalog_lock = json.loads((ROOT / "model-catalog.lock.json").read_text())
        if not self.catalog_path or file_digest(self.catalog_path) != catalog_lock["sha256"]:
            raise RuntimeError("Frozen model catalog is missing or changed")
        pricing = dict(catalog_lock["model"]["cost"])
        # Anthropic's 1-hour cache write is 2x base input, versus 1.25x for 5m.
        pricing["cache_write_1h"] = max(pricing["cache_write"], 2 * pricing["input"])
        private = "/tmp/neoism-public-eval-private"
        request = {"directory": directory, "prompt": instruction, "provider": PROVIDER,
                   "model": MODEL, "effort": protocol["model"]["effort"],
                   "thinking": protocol["model"]["thinking"],
                   "max_output_tokens": protocol["model"]["max_output_tokens"],
                   "timeout_seconds": TIMEOUT_SECONDS, "credential_path": private + "/credential.json",
                   "binary": await self.runtime_binary(), "log_dir": "/logs/agent",
                   "result_path": "/logs/agent/eval-result.json", "api_base_url": API_BASE_URL,
                   "catalog_path": private + "/models.json",
                   "budget": {"maximum_api_cost_usd": admission["maximum_api_cost_usd"],
                              "pricing": pricing, "context_limit": catalog_lock["model"]["limit"]["context"]}}
        self.logs_dir.mkdir(parents=True, exist_ok=True)
        summary = None
        failure = None
        execution = None
        recovery = {"slot": admission["slot"], "plan_sha256": admission["plan_sha256"],
                    "patch_capture": {"status": "not_attempted"}, "private_cleanup": "not_attempted"}
        try:
            result = await self.exec_as_agent(environment, "mkdir -p " + private + " && chmod 700 " + private)
            if result.return_code:
                raise RuntimeError("Cannot prepare private runtime state")
            credential = selected_api_credential(self.auth_path, self.connection_id)
            with tempfile.TemporaryDirectory(prefix="neoism-public-auth-") as temporary:
                for name, value in (("credential.json", credential), ("request.json", request)):
                    local = Path(temporary) / name
                    local.write_text(json.dumps(value, allow_nan=False))
                    local.chmod(0o600)
                    await self._upload_agent_owned_file(environment, local, private + "/" + name)
            await self._upload_agent_owned_file(environment, Path(self.catalog_path), private + "/models.json")
            result = await self.exec_as_agent(environment, "chmod 600 " + private + "/*.json")
            if result.return_code:
                raise RuntimeError("Cannot protect private request files")
            execution = await self.exec_as_agent(
                environment, command=shlex.join(["python3", REMOTE + "/" + self.runtime_name + ".py", "--request", private + "/request.json"]),
                cwd=directory, timeout_sec=TIMEOUT_SECONDS + 45)
        except BaseException as error:
            # Recover results/patch even when Harbor cancels the agent on timeout.
            failure = error
            recovery["execution_error_type"] = type(error).__name__
        finally:
            try:
                with tempfile.TemporaryDirectory(prefix="neoism-public-result-") as temporary:
                    local = Path(temporary) / "result.json"
                    await environment.download_file(request["result_path"], local)
                    summary = json.loads(local.read_text())
                populate_context(context, summary)
                (self.logs_dir / "eval-result.json").write_text(json.dumps(summary, indent=2, allow_nan=False) + "\n")
            except Exception as error:
                recovery["result_recovery_error_type"] = type(error).__name__
                if failure is None:
                    failure = RuntimeError("Native result/accounting artifact unavailable")
            try:
                captured = await environment.exec(_CAPTURE, user="root", timeout_sec=120)
                recovery["patch_capture"] = {"status": "passed" if captured.return_code == 0 else "failed",
                                             "return_code": captured.return_code}
                if captured.return_code == 0:
                    await environment.download_file("/logs/agent/model.patch", self.logs_dir / "model.patch")
                    recovery["patch_capture"]["sha256"] = file_digest(self.logs_dir / "model.patch")
                elif failure is None:
                    failure = RuntimeError("Partial-patch capture failed")
            except Exception as error:
                recovery["patch_capture"] = {"status": "failed", "error_type": type(error).__name__}
                if failure is None:
                    failure = RuntimeError("Partial-patch capture failed")
            try:
                cleaned = await environment.exec("rm -rf " + private, user="root", timeout_sec=15)
                recovery["private_cleanup"] = "passed" if cleaned.return_code == 0 else "failed"
            except Exception as error:
                recovery["private_cleanup"] = "failed"
                recovery["cleanup_error_type"] = type(error).__name__
            (self.logs_dir / "recovery.json").write_text(json.dumps(recovery, indent=2) + "\n")
            metadata = context.metadata or {}
            metadata["recovery"] = recovery
            context.metadata = metadata
        if failure is not None:
            raise failure
        if summary is None or execution is None:
            raise RuntimeError("Native execution did not produce recoverable accounting")
        if summary.get("model_mismatch") or set(summary.get("observed_models", [])) != {MODEL}:
            raise RuntimeError("Same-model audit failed")
        estimate = summary.get("estimated_cost_usd")
        reserved = summary.get("reserved_cost_usd")
        if (type(estimate) not in (int, float) or not math.isfinite(estimate) or estimate < 0
                or type(reserved) not in (int, float) or not math.isfinite(reserved) or reserved < 0
                or estimate + reserved > admission["maximum_api_cost_usd"] + 1e-9):
            raise RuntimeError("Invalid or excessive catalog-price budget accounting")
        if summary.get("accounting_complete") is not True:
            raise RuntimeError("Provider accounting is incomplete; stop further inference")
        if recovery["patch_capture"]["status"] != "passed" or recovery["private_cleanup"] != "passed":
            raise RuntimeError("Sandbox recovery/cleanup failed")
        if execution.return_code or summary.get("status") != "completed":
            # The runner still regrades a captured partial patch in a fresh sandbox.
            raise RuntimeError("Native harness did not complete: " + str(summary.get("status")))
