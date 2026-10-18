"""Native daemon adapter. No agent loop or verifier is implemented here."""
import argparse
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import tempfile
import time
import urllib.parse
import urllib.request

from runtime_utils import load_request, redact, write_result, worker_main
from anthropic_gateway import AnthropicGateway, apply_accounting

MODEL = "claude-sonnet-5-5"
AGENTS = ("build", "plan", "general", "explore", "compaction", "title", "summary")
REASONING = {"thinking": {"type": "adaptive"}, "output_config": {"effort": "high"}}


def validate_request(req):
    if (req.get("model") != MODEL or req.get("provider") != "anthropic"
            or req.get("effort") != "high" or req.get("thinking") != {"type": "adaptive"}):
        raise ValueError("Only direct Anthropic Sonnet 5.5 adaptive/high is allowed")
    base = req.get("api_base_url", "https://api.anthropic.com/v1").rstrip("/")
    url = urllib.parse.urlparse(base)
    mock = req.get("test_mode") is True and req.get("mock_nonce") == "offline-native-wire-test"
    if base != "https://api.anthropic.com/v1" and not (
            mock and url.scheme == "http" and url.hostname in ("127.0.0.1", "::1") and not url.username):
        raise ValueError("Non-Anthropic endpoint is permitted only for nonce-gated loopback tests")
    timeout = req.get("timeout_seconds", 3000)
    if type(timeout) not in (int, float) or not 0 < timeout <= 3000:
        raise ValueError("Deadline must be positive and at most 3000 seconds")
    cap = req.get("max_output_tokens", 32000)
    if type(cap) is not int or not 0 < cap <= 32000:
        raise ValueError("Output token ceiling must be at most 32000")
    return base, timeout


def isolated_env(home):
    # Deliberate allowlist: no real auth, proxy, SDK routing, inherited config or
    # telemetry environment can leak into this child.
    env = {k: os.environ[k] for k in ("PATH", "LANG", "LC_ALL", "TERM", "SSL_CERT_FILE", "SSL_CERT_DIR") if k in os.environ}
    env.update(HOME=str(home), XDG_CONFIG_HOME=str(home / "config"),
               XDG_DATA_HOME=str(home / "data"), XDG_STATE_HOME=str(home / "state"),
               XDG_CACHE_HOME=str(home / "cache"))
    return env


def native_config(base, max_output_tokens=32000, context_tokens=200000):
    tools = {"webfetch": False, "websearch": False, "question": False}
    return {"agent": {
        "model": "anthropic/" + MODEL, "smallModel": "anthropic/" + MODEL,
        "defaultAgent": "build", "enabledProviders": ["anthropic"],
        "permission": {"*": "allow", "external_directory": "allow", "question": "deny", "webfetch": "deny", "websearch": "deny"},
        "tools": tools, "mcp": {}, "autoupdate": False,
        "provider": {"anthropic": {"npm": "@ai-sdk/anthropic", "auth": "required", "options": {"baseURL": base},
            "models": {MODEL: {"id": MODEL, "reasoning": True,
                "limit": {"context": context_tokens, "output": max_output_tokens},
                "options": dict(REASONING, max_tokens=max_output_tokens)}}}},
        "agent": {name: {"model": "anthropic/" + MODEL, "options": REASONING,
                            "tools": tools, "permission": {"*": "allow", "external_directory": "allow", "question": "deny", "webfetch": "deny", "websearch": "deny"}}
                  for name in AGENTS}}}


def frozen_catalog(base, max_output_tokens=32000):
    """Local models.dev-format routing snapshot; no prices or online lookup."""
    return {"anthropic": {"id": "anthropic", "name": "Anthropic", "npm": "@ai-sdk/anthropic",
        "api": base, "env": [], "models": {MODEL: {"id": MODEL, "name": MODEL,
            "release_date": "", "reasoning": True, "tool_call": True,
            "limit": {"context": 200000, "output": max_output_tokens},
            "provider": {"npm": "@ai-sdk/anthropic", "api": base,
                "body": dict(REASONING, max_tokens=max_output_tokens)}}}}}


class NativeAPI:
    def __init__(self, port, directory, deadline):
        self.base = "http://127.0.0.1:" + str(port) + "/v2"
        self.directory = directory
        self.deadline = deadline

    def request(self, method, path, body=None):
        separator = "&" if "?" in path else "?"
        url = self.base + path + separator + urllib.parse.urlencode({"directory": self.directory})
        data = None if body is None else json.dumps(body).encode()
        request = urllib.request.Request(url, data=data, method=method,
                                         headers={"Content-Type": "application/json"})
        remaining = self.deadline - time.monotonic()
        if remaining <= 0:
            raise TimeoutError("Native API accounting deadline expired")
        with urllib.request.urlopen(request, timeout=min(10, remaining)) as response:
            payload = bytearray()
            while True:
                if time.monotonic() >= self.deadline:
                    raise TimeoutError("Native API accounting deadline expired")
                chunk = response.read1(65536)
                if not chunk:
                    break
                payload.extend(chunk)
            return json.loads(payload) if payload else None

    def collect(self, root, sessions):
        pending, seen = [root], set()
        while pending:
            sid = pending.pop()
            if sid in seen:
                continue
            seen.add(sid)
            collected, cursor, cursors = [], None, set()
            while True:
                query = {"limit": 256, "order": "asc"}
                if cursor:
                    query["cursor"] = cursor
                page = self.request("GET", "/sessions/" + sid + "/messages?" + urllib.parse.urlencode(query))
                collected.extend(page["items"])
                sessions[sid] = collected
                cursor = page["cursor"].get("next")
                if not cursor:
                    break
                if cursor in cursors:
                    raise RuntimeError("Repeated transcript cursor")
                cursors.add(cursor)
            children = self.request("GET", "/sessions/" + sid + "/children")
            if children["cursor"].get("next"):
                raise RuntimeError("Unsupported child pagination; refusing incomplete audit")
            pending.extend(child["id"] for child in children["items"])

    def interactions(self, family, service=True):
        snapshot = {}
        for kind in ("permissions", "questions"):
            pending = self.request("GET", "/interactions/" + kind)
            if not isinstance(pending, list):
                raise RuntimeError("Invalid interaction snapshot")
            snapshot[kind] = [item for item in pending if item.get("sessionId") in family]
            if service:
                for item in snapshot[kind]:
                    path = "/interactions/" + kind + "/" + urllib.parse.quote(item["id"], safe="")
                    self.request("POST", path + ("/reply" if kind == "permissions" else "/reject"),
                                 {"reply": "once"} if kind == "permissions" else None)
        return snapshot


def family_finished(runtime):
    if not isinstance(runtime, dict) or not isinstance(runtime.get("runningBackgroundTasks"), list):
        raise RuntimeError("Missing authoritative family runtime snapshot")
    execution = runtime.get("execution")
    if execution is None:
        return False
    if type(execution.get("finished")) is not bool or not isinstance(execution.get("activeSegments"), dict):
        raise RuntimeError("Invalid family execution snapshot")
    return execution["finished"] and not execution["activeSegments"] and not runtime["runningBackgroundTasks"]


def audit_usage(sessions):
    models, providers, tokens, errors, records = set(), set(), {}, [], []
    cost, cost_complete = 0.0, True
    for sid, messages in sessions.items():
        for message in messages:
            info = message["info"]
            if info.get("role") != "assistant":
                continue
            model, provider = info.get("modelId"), info.get("providerId")
            if model:
                models.add(model)
            if provider:
                providers.add(provider)
            if model != MODEL or provider != "anthropic":
                raise RuntimeError("Native family model/provider drift or missing model audit")
            if info.get("error"):
                errors.append(info["error"])
            value = info.get("cost")
            if type(value) in (int, float) and value >= 0:
                cost += value
            else:
                cost_complete = False
            usage = info.get("tokens", {})
            records.append({"session_id": sid, "message_id": info.get("id"), "model": model,
                            "provider": provider, "tokens": usage, "cost": info.get("cost")})
            for key in ("input", "output", "reasoning"):
                tokens[key] = tokens.get(key, 0) + usage.get(key, 0)
            for key, value in usage.get("cache", {}).items():
                tokens["cache_" + key] = tokens.get("cache_" + key, 0) + value
    tokens["input_total"] = tokens.get("input", 0) + tokens.get("cache_read", 0) + tokens.get("cache_write", 0)
    tokens["output_total"] = tokens.get("output", 0)
    return {"observed_models": sorted(models), "observed_providers": sorted(providers),
            "tokens": tokens, "usage_records": records, "native_errors": errors,
            "reported_cost_usd": cost if records and cost_complete else None,
            "usage_metered": bool(tokens["input_total"] + tokens["output_total"])}


def terminate_group(process):
    # Descendants can outlive the leader. Always signal the entire session.
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        pass
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    process.wait()


def run(req, credential):
    base, timeout = validate_request(req)
    key = credential.get("key")
    if credential.get("type") != "api" or not isinstance(key, str) or not key:
        raise ValueError("Fresh Anthropic API projection required")
    if req.get("test_mode") and key != "offline-dummy-key":
        raise ValueError("Mock mode accepts only the fixed dummy credential")
    secrets = [key]
    log_dir = Path(req.get("log_dir", "/logs/agent"))
    log_dir.mkdir(parents=True, exist_ok=True)
    start = time.monotonic()
    result = {"agent": "neoism", "model": MODEL, "provider": "anthropic", "effort": "high",
              "thinking": {"type": "adaptive"}, "status": "error", "timed_out": False}
    process, root, api, sessions = None, None, None, {}
    gateway = None
    try:
        with AnthropicGateway(req, key, start + timeout) as gateway, tempfile.TemporaryDirectory(prefix="neoism-eval-") as private:
            base = gateway.url
            secrets.append(gateway.native_key)
            home = Path(private)
            config = home / "config"
            # Standalone 0.7.128 uses StandardConfigSourceService, NOT the
            # desktop's grouped config.json/NEOISM_AGENT_CONFIG_DIR projection.
            standalone = config / "agent"
            standalone.mkdir(parents=True)
            catalog = req.get("catalog_path")
            context_tokens = req["budget"]["context_limit"]
            if catalog:
                snapshot = json.loads(Path(catalog).read_text())
                metadata = snapshot["anthropic"]["models"][MODEL]
                result["catalog_source"] = "supplied_frozen_snapshot"
                result["catalog_pricing_available"] = metadata.get("cost") is not None
            else:
                snapshot = frozen_catalog(base, req.get("max_output_tokens", 32000))
                metadata = snapshot["anthropic"]["models"][MODEL]
                result["catalog_source"] = "embedded_routing_snapshot"
                result["catalog_pricing_available"] = False
            # Never let a supplied routing catalog bypass the private relay.
            snapshot["anthropic"]["api"] = base
            metadata["limit"] = {"context": context_tokens, "output": req.get("max_output_tokens", 32000)}
            metadata["provider"] = {"npm": "@ai-sdk/anthropic", "api": base,
                "body": dict(REASONING, max_tokens=req.get("max_output_tokens", 32000))}
            catalog = str(home / "models.json")
            Path(catalog).write_text(json.dumps(snapshot))
            (standalone / "agent.json").write_text(json.dumps(native_config(base, req.get("max_output_tokens", 32000), context_tokens)["agent"]))
            env = isolated_env(home)
            env.update(NEOISM_AGENT_AUTH_CONTENT=json.dumps({"anthropic": {"type": "api", "key": gateway.native_key}}),
                       NEOISM_AGENT_CONFIG_DIR=str(config), NEOISM_AGENT_STATE_DIR=str(home / "state"),
                       NEOISM_AGENT_DISABLE_MODELS_FETCH="true", NEOISM_AGENT_MODELS_PATH=catalog,
                       NEOISM_AGENT_BASE_URL_ANTHROPIC=base)
            with socket.socket() as sock:
                sock.bind(("127.0.0.1", 0))
                port = sock.getsockname()[1]
            deadline = gateway.deadline
            api = NativeAPI(port, req["directory"], deadline)
            # Anonymous temporary streams avoid unsanitized persisted server logs.
            with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
                process = subprocess.Popen([req.get("binary", "/opt/neoism-public-eval/bin/neoism-agent"),
                    "serve", "--hostname", "127.0.0.1", "--port", str(port)],
                    cwd=req["directory"], env=env, stdout=stdout, stderr=stderr, start_new_session=True)
                try:
                    ready = min(deadline, start + 45)
                    while True:
                        if process.poll() is not None:
                            raise RuntimeError("Native server exited before readiness; inspect version/ABI")
                        try:
                            api.request("GET", "/health")
                            break
                        except OSError:
                            if time.monotonic() >= ready:
                                raise TimeoutError("Native server readiness timed out")
                            time.sleep(.1)
                    effective = api.request("GET", "/config")
                    expected = native_config(base, req.get("max_output_tokens", 32000), context_tokens)["agent"]
                    for field in ("model", "smallModel", "enabledProviders", "permission", "tools"):
                        if effective.get(field) != expected[field]:
                            raise RuntimeError("Native effective configuration drift: " + field + " actual=" + repr(effective.get(field)) + " keys=" + repr(list(effective)))
                    for name in AGENTS:
                        actual = effective.get("agent", {}).get(name, {})
                        if actual.get("model") != "anthropic/" + MODEL or actual.get("options") != REASONING:
                            raise RuntimeError("Native effective agent route/reasoning drift: " + name)
                    result["effective_routes_verified"] = list(AGENTS)
                    root = api.request("POST", "/sessions", {"title": "official-swebench-pro-v2",
                        "permission": [{"permission": "*", "pattern": "*", "action": "allow"},
                            {"permission": "external_directory", "pattern": "*", "action": "allow"}]})["id"]
                    api.request("POST", "/sessions/" + root + "/prompt", {
                        "prompt": req["prompt"], "model": {"providerId": "anthropic", "modelId": MODEL},
                        "agent": "build", "variant": "high",
                        "tools": {"webfetch": False, "websearch": False, "question": False}})
                    while time.monotonic() < deadline:
                        api.collect(root, sessions)
                        result.update(audit_usage(sessions))
                        api.interactions(set(sessions) | {root})
                        runtime = api.request("GET", "/sessions/" + root + "/runtime")
                        if family_finished(runtime):
                            result["status"] = "completed"
                            break
                        time.sleep(.25)
                    else:
                        result["timed_out"] = True
                        result["status"] = "timeout"
                finally:
                    # Snapshot BEFORE cancellation; a fresh bounded cleanup window is
                    # accounting only, never another inference/child execution timeout.
                    if root:
                        api.deadline = time.monotonic() + 20
                        try:
                            api.collect(root, sessions)
                            result["final_runtime"] = api.request("GET", "/sessions/" + root + "/runtime")
                            result["final_interactions"] = api.interactions(set(sessions) | {root}, service=False)
                            result.update(audit_usage(sessions))
                        except Exception as error:
                            result.setdefault("collection_errors", []).append(str(error))
                            result["status"] = "error"
                        for sid in set(sessions) | {root}:
                            try:
                                api.request("POST", "/sessions/" + sid + "/abort")
                            except Exception as error:
                                result.setdefault("abort_errors", []).append(str(error))
                        if result.get("status") != "completed" and not result.get("abort_errors"):
                            try:
                                while time.monotonic() < api.deadline:
                                    cancelled = api.request("GET", "/sessions/" + root + "/runtime")
                                    result["cancelled_runtime"] = cancelled
                                    if family_finished(cancelled):
                                        break
                                    time.sleep(.1)
                            except Exception as error:
                                result.setdefault("abort_errors", []).append(str(error))
                    terminate_group(process)
                    process = None
                    for stream, name in ((stdout, "native.stdout.log"), (stderr, "native.stderr.log")):
                        stream.seek(0)
                        (log_dir / name).write_text(redact(stream.read().decode(errors="replace"), secrets))
    except Exception as error:
        result["error"] = redact(str(error), secrets)
        result["timed_out"] = isinstance(error, TimeoutError) or time.monotonic() >= (gateway.deadline if gateway else start + timeout)
        result["status"] = "timeout" if result["timed_out"] else "error"
    finally:
        if process:
            terminate_group(process)
        result["duration_seconds"] = time.monotonic() - start
        if not result.get("catalog_pricing_available"):
            result["reported_cost_usd"] = None
        result["sessions"] = sessions
        if result.get("native_errors") or not result.get("usage_records") or not result.get("usage_metered"):
            if result["status"] == "completed":
                result["status"] = "error"
        lifecycle = False
        try:
            lifecycle = ((result.get("status") == "completed" and family_finished(result.get("final_runtime"))
                          and not result.get("collection_errors"))
                         or (result.get("cancelled_runtime") is not None
                             and family_finished(result["cancelled_runtime"]) and not result.get("abort_errors")))
        except (RuntimeError, TypeError, AttributeError):
            pass
        apply_accounting(result, gateway, lifecycle)
        write_result(req["result_path"], json.loads(redact(json.dumps(result), secrets)))
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--request", required=True)
    args = parser.parse_args()
    worker_main(args.request, run)


if __name__ == "__main__":
    main()
