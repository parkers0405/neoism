"""OpenCode 2.0.32 native standalone loop; all inference uses shared relay.

Source contract audited at anomalyco/opencode
843a6c514e606b436a7fe713422ace0a93ad3d61. Native CLI events retain
foreground-child audits, but never establish background-child completion.
"""
from __future__ import annotations
import argparse
import hashlib
import json
from pathlib import Path
import tempfile
import time
from urllib.parse import urlsplit

from anthropic_gateway import AnthropicGateway, SSEAudit, check_wire_request, token_count, apply_accounting
from runtime_utils import run_logged, write_result, worker_main, redact

VERSION = "2.0.32"
NATIVE_SHA256 = "489b51816a77809e841310e80c88e834c28c19ad1639e12ba8a19f08b47ab607"
MODEL = "claude-sonnet-5-5"
MODEL_REF = "anthropic/" + MODEL
BINARY = "/opt/neoism-public-eval/bin/opencode-v2"
AGENTS = ("build", "general", "explore", "compaction", "title", "summary")


def validate_request(request: dict) -> None:
    for key, value in (("model", MODEL), ("provider", "anthropic"),
                       ("effort", "high"), ("thinking", {"type": "adaptive"})):
        if request.get(key) != value:
            raise ValueError("Locked OpenCode V2 request mismatch: " + key)
    cap = request.get("max_output_tokens", 32000)
    if type(cap) is not int or not 0 < cap <= 32000:
        raise ValueError("max_output_tokens ceiling is 32000")
    timeout = request.get("timeout_seconds", 3000)
    if type(timeout) not in (int, float) or not 0 < timeout <= 3000:
        raise ValueError("timeout_seconds must be in (0, 3000]")
    endpoint = urlsplit(request.get("api_base_url", "https://api.anthropic.com/v1"))
    official = endpoint.scheme == "https" and endpoint.netloc == "api.anthropic.com" and endpoint.path.rstrip("/") == "/v1"
    mock = request.get("test_mode") is True and request.get("mock_nonce") == "offline-native-wire-test"
    fixture = mock and endpoint.scheme == "http" and endpoint.hostname in ("127.0.0.1", "::1", "localhost") and endpoint.path.rstrip("/") == "/v1"
    if not (official or fixture) or endpoint.username or endpoint.password or endpoint.query or endpoint.fragment:
        raise ValueError("Only direct Anthropic or explicitly gated loopback fixtures allowed")


def build_config(request: dict, relay_url=None) -> dict:
    """Validate ORIGINAL endpoint, then apply private relay as internal routing."""
    validate_request(request)
    body = {"thinking": {"type": "adaptive"}, "output_config": {"effort": "high"},
            "max_tokens": request.get("max_output_tokens", 32000)}
    permissions = [{"action": "*", "resource": "*", "effect": "allow"}]
    permissions += [{"action": action, "resource": "*", "effect": "deny"}
                    for action in ("webfetch", "websearch", "browser", "mcp*", "question")]
    return {
        "model": MODEL_REF, "default_agent": "build", "update": "disable",
        "share": "disabled", "warming": False,
        "providers": {"anthropic": {
            "env": ["ANTHROPIC_API_KEY"],
            "settings": {"baseURL": relay_url or request.get("api_base_url", "https://api.anthropic.com/v1")},
            "body": body, "models": {MODEL: {"body": body}},
        }},
        "agents": {name: {"model": MODEL_REF, "request": {"body": body},
                          "permissions": permissions} for name in AGENTS},
        "permissions": permissions,
        "policies": [{"action": "provider.use", "resource": "*", "effect": "deny"},
                     {"action": "provider.use", "resource": "anthropic", "effect": "allow"}],
        "mcp": {}, "plugins": [], "skills": [], "instructions": [],
    }


def build_command(request: dict) -> list[str]:
    validate_request(request)
    if not isinstance(request.get("prompt"), str) or not request["prompt"]:
        raise ValueError("An unchanged nonempty official prompt is required")
    return [request.get("binary") or BINARY, "run", "--standalone", "--auto",
            "--format", "json", "--model", MODEL_REF, "--agent", "build",
            "--", request["prompt"]]


def build_environment(request: dict, private_root: str, selected_key: str, relay_url=None) -> dict[str, str]:
    root = Path(private_root).resolve()
    return {
        "PATH": "/usr/local/bin:/usr/bin:/bin", "HOME": str(root / "home"),
        "XDG_CONFIG_HOME": str(root / "config"), "XDG_DATA_HOME": str(root / "data"),
        "XDG_CACHE_HOME": str(root / "cache"), "XDG_STATE_HOME": str(root / "state"),
        "OPENCODE_CONFIG_DIR": str(root / "config" / "opencode"),
        "OPENCODE_CONFIG_PROJECT_DISABLE": "1", "OPENCODE_DISABLE_CLAUDE_CODE": "1",
        "OPENCODE_DISABLE_MODELS_FETCH": "1", "OPENCODE_DISABLE_FILEWATCHER": "1",
        "OPENCODE_CONFIG_CONTENT": json.dumps(build_config(request, relay_url)),
        "ANTHROPIC_API_KEY": selected_key, "NO_COLOR": "1",
    }


def native_audit(path: str) -> dict:
    """Root counters are partial; preserve native child lifecycle evidence."""
    steps, errors, sessions, tools, events = [], 0, set(), set(), 0
    completed_children = unverified_children = 0
    projected = {key: 0 for key in ("input_tokens", "output_tokens", "cache_read_input_tokens", "cache_creation_input_tokens")}
    with Path(path).open() as stream:
        for line in stream:
            if not line.strip():
                continue
            event = json.loads(line)
            events += 1
            if event.get("sessionID"):
                sessions.add(event["sessionID"])
            if event.get("type") == "error":
                errors += 1
            if event.get("type") == "step_finish":
                tokens = event["part"].get("tokens")
                steps.append(tokens)
                if isinstance(tokens, dict):
                    for key, value in (("input_tokens", tokens["input"]), ("output_tokens", tokens["output"]),
                                       ("cache_read_input_tokens", tokens["cache"]["read"]),
                                       ("cache_creation_input_tokens", tokens["cache"]["write"])):
                        projected[key] += token_count(value)
            if event.get("type") == "tool_use":
                part = event.get("part", {})
                tools.add(part.get("tool", "unknown"))
                if part.get("tool") == "subagent":
                    state = part.get("state", {})
                    metadata = state.get("metadata", {}).get("metadata", {})
                    if (not state.get("input", {}).get("background") and state.get("status") == "completed"
                            and metadata.get("status") == "completed"):
                        completed_children += 1
                    else:
                        unverified_children += 1
    return {"event_count": events, "step_count": len(steps), "error_count": errors, "session_count": len(sessions),
            "tool_count": len(tools), "steps_with_usage": sum(isinstance(x, dict) for x in steps),
            "projected_root_usage": projected, "projected_root_usage_may_be_partial": True,
            "completed_foreground_children": completed_children, "unverified_children": unverified_children}


def summarize(request: dict, execution: dict, calls: list[dict], native: dict) -> dict:
    unverified_children = bool(native.get("unverified_children") or any(c.get("background_child_seen") for c in calls))
    complete = (bool(calls) and all(call["complete"] and call["request_checked"] for call in calls)
                and not unverified_children and native.get("event_count", 0) > 0
                and not native["error_count"] and native["steps_with_usage"] == native["step_count"])
    mismatch = any(call.get("error") in ("request_model_mismatch", "response_model_mismatch") for call in calls)
    totals = {key: sum(call["usage"].get(key, 0) for call in calls if call.get("usage"))
              for key in ("input_tokens", "output_tokens", "cache_read_input_tokens", "cache_creation_input_tokens")}
    totals["input_total"] = totals["input_tokens"] + totals["cache_read_input_tokens"] + totals["cache_creation_input_tokens"]
    totals["output_total"] = totals["output_tokens"]
    succeeded = (complete and not mismatch and execution["returncode"] == 0 and not execution["timed_out"]
                 and not execution.get("log_limited"))
    return {"status": "completed" if succeeded else "failed", "version": VERSION,
            "model": MODEL, "provider": "anthropic", "effort": "high", "thinking": {"type": "adaptive"},
            "max_output_tokens": request.get("max_output_tokens", 32000), "accounting_complete": complete,
            "model_mismatch": mismatch, "observed_models": sorted({c["model"] for c in calls if c.get("model")}),
            "usage": totals if complete else None, "partial_usage": totals if not complete else None,
            "provider_calls": calls, "native_audit": native, "inference_started": any(c.get("forwarded") for c in calls),
            "blockers": (["Background/incomplete child lifecycle is unverified"] if unverified_children else [])
                        + sorted({call["error"] for call in calls if call.get("error")}),
            "accounting_scope": "all requests through the isolated pinned Anthropic provider, including auxiliary calls",
            **execution}


def run(request: dict, credential: dict) -> dict:
    start = time.monotonic()
    gateway = None
    key = credential.get("key", "")
    result = {"status": "failed", "returncode": 78, "agent": "opencode_v2", "timed_out": False}
    try:
        validate_request(request)
        if credential.get("type") != "api" or not isinstance(key, str) or not key:
            raise ValueError("Fresh API projection required")
        with Path(request.get("binary") or BINARY).open("rb") as artifact:
            digest = hashlib.sha256()
            for chunk in iter(lambda: artifact.read(1024 * 1024), b""):
                digest.update(chunk)
            if digest.hexdigest() != NATIVE_SHA256:
                raise ValueError("Runtime binary does not match locked V2 artifact")
        with AnthropicGateway(request, key, start + request["timeout_seconds"]) as gateway, tempfile.TemporaryDirectory(prefix="neoism-opencode-v2-") as root:
            env = build_environment(request, root, gateway.native_key, gateway.url)
            execution = run_logged(build_command(request), env, request["directory"],
                                   max(.001, gateway.deadline - time.monotonic()), request["log_dir"], [key, gateway.native_key])
        try:
            native = native_audit(execution["stdout_path"])
        except (ValueError, KeyError, OSError, TypeError):
            native = {"step_count": 0, "error_count": 1, "session_count": 0, "steps_with_usage": 0}
        result.update(summarize(request, execution, gateway.snapshot(), native))
    except Exception as error:
        result["error"] = redact(str(error), [key])
        result["timed_out"] = time.monotonic() >= start + request.get("timeout_seconds", 3000)
    finally:
        native = result.get("native_audit", {})
        lifecycle = ("stdout_path" in result and not native.get("unverified_children")
                     and gateway is not None and not any(c.get("background_child_seen") for c in gateway.snapshot()))
        apply_accounting(result, gateway, lifecycle)
        result["duration_seconds"] = time.monotonic() - start
        result = json.loads(redact(json.dumps(result), [key]))
        write_result(str(Path(request.get("log_dir", "/logs/agent")) / "opencode-v2-audit.json"), result)
        write_result(request["result_path"], result)
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--request", required=True)
    args = parser.parse_args()
    result = worker_main(args.request, run)
    return 0 if result["status"] == "completed" else 78


if __name__ == "__main__":
    raise SystemExit(main())
