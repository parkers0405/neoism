"""Claude Code's native print/stream protocol, not a replacement agent loop."""
import argparse
import json
import os
from pathlib import Path
import tempfile
import time
import urllib.parse

from runtime_utils import load_request, redact, run_logged, write_result, worker_main
from anthropic_gateway import AnthropicGateway, apply_accounting

MODEL = "claude-sonnet-5-5"


def validate_request(req):
    if (req.get("model") != MODEL or req.get("provider") != "anthropic"
            or req.get("effort") != "high" or req.get("thinking") != {"type": "adaptive"}):
        raise ValueError("Only direct Anthropic Sonnet 5.5 adaptive/high is allowed")
    base = req.get("api_base_url", "https://api.anthropic.com/v1").rstrip("/")
    url = urllib.parse.urlparse(base)
    mock = req.get("test_mode") is True and req.get("mock_nonce") == "offline-native-wire-test"
    if base != "https://api.anthropic.com/v1" and not (
            mock and url.scheme == "http" and url.hostname in ("127.0.0.1", "::1") and not url.username):
        raise ValueError("Non-Anthropic endpoint requires nonce-gated loopback test mode")
    timeout = req.get("timeout_seconds", 3000)
    if type(timeout) not in (int, float) or not 0 < timeout <= 3000:
        raise ValueError("Deadline must be positive and at most 3000 seconds")
    cap = req.get("max_output_tokens", 32000)
    if type(cap) is not int or not 0 < cap <= 32000:
        raise ValueError("Output token ceiling must be at most 32000")
    return base, timeout


def child_env(home, key, base, max_output_tokens=32000):
    env = {k: os.environ[k] for k in ("PATH", "LANG", "LC_ALL", "TERM", "SSL_CERT_FILE", "SSL_CERT_DIR") if k in os.environ}
    env.update(HOME=str(home), XDG_CONFIG_HOME=str(home / "config"),
               XDG_DATA_HOME=str(home / "data"), XDG_STATE_HOME=str(home / "state"),
               XDG_CACHE_HOME=str(home / "cache"), CLAUDE_CONFIG_DIR=str(home / "claude"),
               ANTHROPIC_API_KEY=key, ANTHROPIC_BASE_URL=base.removesuffix("/v1"),
               ANTHROPIC_MODEL=MODEL, ANTHROPIC_DEFAULT_SONNET_MODEL=MODEL,
               ANTHROPIC_DEFAULT_OPUS_MODEL=MODEL, ANTHROPIC_DEFAULT_HAIKU_MODEL=MODEL,
               ANTHROPIC_SMALL_FAST_MODEL=MODEL, CLAUDE_CODE_SUBAGENT_MODEL=MODEL,
               CLAUDE_CODE_MAX_OUTPUT_TOKENS=str(max_output_tokens),
               CLAUDE_CODE_EFFORT_LEVEL="high", CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC="1",
               DISABLE_AUTOUPDATER="1", DISABLE_TELEMETRY="1", DISABLE_ERROR_REPORTING="1",
               CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY="1", CLAUDE_CODE_DISABLE_AUTO_MEMORY="1",
               IS_SANDBOX="1")
    return env


def command(binary, settings, prompt):
    # Do not use --bare: it silently reduces tools to Bash/Edit/Read, removing
    # native subagents. Private HOME + safe-mode/settings/MCP isolate the full CLI.
    return [binary, "--safe-mode", "--print", "--verbose",
            "--output-format", "stream-json", "--include-partial-messages", "--forward-subagent-text",
            "--model", MODEL, "--effort", "high", "--dangerously-skip-permissions",
            "--no-chrome", "--disable-slash-commands", "--setting-sources", "",
            "--settings", str(settings), "--strict-mcp-config", "--mcp-config", '{"mcpServers":{}}',
            "--disallowedTools", "WebFetch,WebSearch", "--", prompt]


def audit_stream(path, partial=None):
    models, records, final = set(), [], None
    partial = {} if partial is None else partial
    partial["usage_records"] = records
    with Path(path).open() as stream:
        for line in stream:
            if not line.strip():
                continue
            item = json.loads(line)
            kind = item.get("type")
            if kind == "assistant":
                message = item["message"]
                model = message.get("model")
                # Synthetic error messages are not inference and do not carry a
                # model; retain them as errors rather than treating as usage.
                if model and not model.startswith("<"):
                    models.add(model)
                    records.append({"model": model, "usage": message.get("usage"),
                                    "message_id": message.get("id"),
                                    "parent_tool_use_id": item.get("parent_tool_use_id")})
            elif kind == "stream_event":
                message = item.get("event", {}).get("message", {})
                if message.get("model"):
                    models.add(message["model"])
            elif kind == "result":
                final = item
                models.update(item.get("modelUsage", {}).keys())
            partial["observed_models"] = sorted(models)
    if models - {MODEL}:
        partial["model_mismatch"] = True
        raise RuntimeError("Claude native root/subagent/fast model drift: " + repr(sorted(models)))
    if final is None:
        raise RuntimeError("Missing Claude terminal result (partial stream retained)")
    # Preserve native field names. Do not sum assistant incremental usage on top
    # of the terminal aggregate or confuse duration_ms with duration_api_ms.
    return {"observed_models": sorted(models), "usage_records": records,
            "usage": final.get("usage"), "modelUsage": final.get("modelUsage"),
            "total_cost_usd": final.get("total_cost_usd"), "duration_ms": final.get("duration_ms"),
            "duration_api_ms": final.get("duration_api_ms"), "num_turns": final.get("num_turns"),
            "native_result": final, "native_is_error": final.get("is_error", False)}


def run(req, credential):
    base, timeout = validate_request(req)
    key = credential.get("key")
    if credential.get("type") != "api" or not isinstance(key, str) or not key:
        raise ValueError("Fresh Anthropic API projection required")
    if req.get("test_mode") and key != "offline-dummy-key":
        raise ValueError("Mock mode accepts only the fixed dummy credential")
    start = time.monotonic()
    result = {"agent": "claude_code", "model": MODEL, "provider": "anthropic", "effort": "high",
              "thinking": {"type": "adaptive"}, "status": "error", "timed_out": False}
    gateway = None
    try:
        with AnthropicGateway(req, key, start + timeout) as gateway, tempfile.TemporaryDirectory(prefix="claude-eval-") as private:
            base = gateway.url
            home = Path(private)
            settings = home / "settings.json"
            settings.write_text(json.dumps({"model": MODEL, "effortLevel": "high",
                "alwaysThinkingEnabled": True, "enabledPlugins": {}, "disableAllHooks": True,
                "permissions": {"deny": ["WebFetch", "WebSearch"]}}))
            execution = run_logged(command(req.get("binary", "/opt/neoism-public-eval/bin/claude"),
                                           settings, req["prompt"]),
                child_env(home, gateway.native_key, base, req.get("max_output_tokens", 32000)), req["directory"], max(.001, gateway.deadline - time.monotonic()), req.get("log_dir", "/logs/agent"), [key, gateway.native_key])
            result.update(execution)
            try:
                result.update(audit_stream(execution["stdout_path"], result))
            except Exception as error:
                result["audit_error"] = str(error)
            if execution["timed_out"]:
                result["status"] = "timeout"
            elif (execution["returncode"] == 0 and not result.get("native_is_error", True)
                    and result.get("observed_models") == [MODEL] and "audit_error" not in result):
                result["status"] = "completed"
    except Exception as error:
        result["error"] = redact(str(error), [key])
    finally:
        result["duration_seconds"] = time.monotonic() - start
        # A supervised timeout may have complete provider accounting after the
        # owned process group is stopped; that partial patch still gets graded.
        lifecycle = ("returncode" in result and not any(c.get("background_child_seen")
                     for c in gateway.snapshot())) if gateway else False
        apply_accounting(result, gateway, lifecycle)
        write_result(req["result_path"], json.loads(redact(json.dumps(result), [key])))
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--request", required=True)
    args = parser.parse_args()
    worker_main(args.request, run)


if __name__ == "__main__":
    main()
