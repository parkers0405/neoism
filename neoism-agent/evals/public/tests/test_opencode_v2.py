"""No real keys or external inference. Optional integration uses actual pinned V2."""
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from unittest.mock import patch

PUBLIC = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("opencode_v2_runtime", PUBLIC / "neoism_harbor/runtime/opencode_v2.py")
assert SPEC is not None and SPEC.loader is not None
v2 = importlib.util.module_from_spec(SPEC)
with patch.object(sys, "path", [str(PUBLIC / "neoism_harbor/runtime")] + sys.path):
    SPEC.loader.exec_module(v2)
BINARY = PUBLIC / ".cache/opencode-v2/node_modules/@opencode/cli-linux-x64/bin/opencode"


def request(**updates):
    value = {"model": "claude-sonnet-5-5", "provider": "anthropic", "effort": "high",
             "thinking": {"type": "adaptive"}, "timeout_seconds": 30,
             "directory": "/app", "prompt": "Fixture prompt: do not use tools.",
             "test_mode": True, "mock_nonce": "offline-native-wire-test",
             "budget": {"maximum_api_cost_usd": 100, "context_limit": 1000000,
                        "pricing": {"input": 1, "output": 5, "cache_read": .1,
                                    "cache_write": 1.25, "cache_write_1h": 2}}}
    value.update(updates)
    return value


class ContractTests(unittest.TestCase):
    def test_plural_schema_and_every_route_locked(self):
        cfg = v2.build_config(request())
        self.assertNotIn("provider", cfg)
        self.assertNotIn("agent", cfg)
        self.assertNotIn("small_model", cfg)
        self.assertEqual(set(cfg["agents"]), set(v2.AGENTS))
        for agent in cfg["agents"].values():
            self.assertEqual(agent["model"], v2.MODEL_REF)
            self.assertEqual(agent["request"]["body"]["thinking"], {"type": "adaptive"})
            self.assertEqual(agent["request"]["body"]["output_config"], {"effort": "high"})
            for action in ("webfetch", "websearch", "browser"):
                self.assertIn({"action": action, "resource": "*", "effect": "deny"}, agent["permissions"])
        self.assertEqual(cfg["plugins"], [])
        self.assertEqual(cfg["mcp"], {})

    def test_invalid_lock_rejected(self):
        for key, value in (("model", "other"), ("provider", "bedrock"), ("effort", "low"),
                           ("thinking", {"type": "enabled"}), ("timeout_seconds", 3001),
                           ("api_base_url", "https://example.org/v1"),
                           ("api_base_url", "https://secret@api.anthropic.com/v1")):
            with self.subTest(key=key, value=value), self.assertRaises(ValueError):
                v2.build_config(request(**{key: value}))

    def test_command_preserves_prompt_no_auth_or_server_reuse(self):
        prompt = "--literal\nOfficial prompt unchanged\n"
        command = v2.build_command(request(prompt=prompt))
        self.assertEqual(command[-2:], ["--", prompt])
        self.assertIn("--standalone", command)
        self.assertNotIn("--server", command)
        self.assertNotIn("--continue", command)
        self.assertNotIn("--session", command)

    def test_environment_does_not_inherit_host(self):
        env = v2.build_environment(request(), "/tmp/private-fixture", "synthetic-not-a-key")
        self.assertEqual(env["ANTHROPIC_API_KEY"], "synthetic-not-a-key")
        self.assertNotIn("synthetic-not-a-key", env["OPENCODE_CONFIG_CONTENT"])
        self.assertEqual(env["OPENCODE_CONFIG_PROJECT_DISABLE"], "1")
        self.assertNotIn("OPENCODE_SERVER", env)
        self.assertNotIn("ANTHROPIC_AUTH_TOKEN", env)
        self.assertNotIn("HTTP_PROXY", env)

    def test_unknown_accounting_blocks_before_any_inference(self):
        result = v2.summarize(request(), {"returncode": 0, "timed_out": False}, [],
                              {"step_count": 1, "error_count": 0, "steps_with_usage": 1})
        self.assertEqual(result["status"], "failed")
        self.assertFalse(result["inference_started"])
        self.assertFalse(result["accounting_complete"])
        self.assertIsNone(result["usage"])

    def test_missing_credential_fails_admission_without_launching_cli(self):
        with tempfile.TemporaryDirectory(prefix="opencode-v2-worker-") as root:
            result_path = Path(root) / "result.json"
            req_path = Path(root) / "request.json"
            req_path.write_text(json.dumps(request(
                credential_path=str(Path(root) / "nonexistent-credential.json"),
                prompt="sentinel-never-copy-to-audit", result_path=str(result_path))))
            proc = subprocess.run([sys.executable, str(PUBLIC / "neoism_harbor/runtime/opencode_v2.py"),
                                   "--request", str(req_path)], text=True, capture_output=True, timeout=10)
            self.assertEqual(proc.returncode, 78, proc.stderr)
            result = json.loads(result_path.read_text())
            self.assertFalse(result["inference_started"])
            self.assertIn(result["status"], ("failed", "error"))
            self.assertNotIn("sentinel-never-copy-to-audit", result_path.read_text())

    def test_package_pin_and_integrity_lock(self):
        package = json.loads((PUBLIC / "opencode-v2/package.json").read_text())
        lock = json.loads((PUBLIC / "opencode-v2/package-lock.json").read_text())
        self.assertEqual(package["dependencies"], {"@opencode/cli": "2.0.32"})
        entry = lock["packages"]["node_modules/@opencode/cli"]
        self.assertEqual(entry["version"], "2.0.32")
        self.assertEqual(entry["integrity"], "sha512-nhRlQGFLMoqx50VtIvUmCV/43W6dR8Y+OXzXq17zstSjzLIUdwfBZG6zP3Mpzo0cmLQ2hvXW4/PbhvlLyhwGGg==")


class AccountingTests(unittest.TestCase):
    def _feed(self, audit, event):
        audit.line(("data: " + json.dumps(event) + "\n").encode())
        audit.line(b"\n")

    def _start(self, model=v2.MODEL):
        return {"type": "message_start", "message": {"model": model, "usage": {
            "input_tokens": 11, "output_tokens": 2, "cache_read_input_tokens": 3,
            "cache_creation_input_tokens": 5}}}

    def test_cumulative_usage_not_double_counted(self):
        audit = v2.SSEAudit()
        self._feed(audit, self._start())
        self._feed(audit, {"type": "message_delta", "usage": {"output_tokens": 7}})
        self._feed(audit, {"type": "message_stop"})
        self.assertEqual(audit.finish()["output_tokens"], 7)
        self.assertEqual(audit.finish()["input_tokens"], 11)
        self.assertEqual(audit.finish()["cache_read_input_tokens"], 3)

    def test_missing_usage_or_terminal_event_fails_closed(self):
        for events in ([], [self._start()], [self._start(), {"type": "message_delta", "usage": {"output_tokens": 7}}]):
            with self.subTest(events=len(events)):
                audit = v2.SSEAudit()
                for event in events:
                    self._feed(audit, event)
                with self.assertRaises(ValueError):
                    audit.finish()

    def test_response_model_drift_and_invalid_usage_fail_closed(self):
        with self.assertRaisesRegex(ValueError, "response_model_mismatch"):
            self._feed(v2.SSEAudit(), self._start("other-model"))
        event = self._start()
        event["message"]["usage"]["input_tokens"] = -1
        with self.assertRaises(ValueError):
            self._feed(v2.SSEAudit(), event)

    def test_request_model_drift_is_rejected_before_forwarding(self):
        import http.client
        # Port 9 is never contacted: the request must fail before connection.
        with v2.AnthropicGateway(request(api_base_url="http://127.0.0.1:9/v1"), "offline-dummy-key") as observer:
            endpoint = v2.urlsplit(observer.url)
            client = http.client.HTTPConnection(endpoint.hostname, endpoint.port, timeout=5)
            body = {"model": "other-model", "thinking": {"type": "adaptive"},
                    "output_config": {"effort": "high"}, "max_tokens": 32000, "stream": True}
            client.request("POST", "/v1/messages", json.dumps(body), {"x-api-key": "dummy"})
            response = client.getresponse()
            self.assertEqual(response.status, 502)
            response.read()
            client.close()
        calls = observer.snapshot()
        self.assertFalse(calls[0]["request_checked"])
        self.assertEqual(calls[0]["error"], "request_model_mismatch")

    def test_observer_sees_nested_background_and_off_model_subagent_requests(self):
        for background, model in ((True, None), (False, "anthropic/other-model")):
            with self.subTest(background=background):
                audit = v2.SSEAudit()
                self._feed(audit, self._start())
                self._feed(audit, {"type": "content_block_start", "index": 0,
                                   "content_block": {"type": "tool_use", "name": "subagent", "input": {}}})
                self._feed(audit, {"type": "content_block_delta", "index": 0, "delta": {
                    "type": "input_json_delta", "partial_json": json.dumps({"background": background, "model": model})}})
                stop = {"type": "content_block_stop", "index": 0}
                if model:
                    with self.assertRaisesRegex(ValueError, "request_model_mismatch"):
                        self._feed(audit, stop)
                else:
                    self._feed(audit, stop)
                    self.assertTrue(audit.background_child_seen)

    def test_background_lifecycle_unknown_cannot_claim_complete(self):
        calls = [{"complete": True, "request_checked": True, "model": v2.MODEL, "error": None,
                  "usage": {"input_tokens": 1, "output_tokens": 1, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0}}]
        native = {"event_count": 1, "step_count": 0, "steps_with_usage": 0, "error_count": 0, "unverified_children": 1}
        result = v2.summarize(request(), {"returncode": 0, "timed_out": False}, calls, native)
        self.assertEqual(result["status"], "failed")
        self.assertFalse(result["accounting_complete"])
        self.assertIsNone(result["usage"])
        self.assertTrue(result["blockers"])


class InstallerTests(unittest.IsolatedAsyncioTestCase):
    async def test_frozen_upload_and_async_runtime_binary(self):
        if not BINARY.is_file() or importlib.util.find_spec("harbor") is None:
            self.skipTest("Pinned Harbor venv and private artifact required")
        from types import SimpleNamespace
        from unittest.mock import AsyncMock
        from neoism_harbor.opencode_v2 import OpenCodeV2Sonnet
        agent = object.__new__(OpenCodeV2Sonnet)
        agent._parsed_model_name = v2.MODEL
        agent._parsed_model_provider = "anthropic"
        self.assertTrue(callable(agent.version))
        self.assertEqual(agent.version(), "2.0.32")
        info = agent.to_agent_info()
        self.assertEqual(info.version, "2.0.32")
        self.assertEqual(info.name, "opencode-v2-sonnet-locked")
        agent.native_binary_path = str(BINARY)
        agent.native_binary_sha256 = None
        agent.exec_as_root = AsyncMock(side_effect=[
            SimpleNamespace(return_code=0, stdout=""),
            SimpleNamespace(return_code=0, stdout="opencode v2.0.32\n")])
        environment = SimpleNamespace(upload_file=AsyncMock())
        await agent.install_cli(environment)
        self.assertEqual(await agent.runtime_binary(), "/opt/neoism-public-eval/bin/opencode-v2")
        environment.upload_file.assert_awaited_once_with(source_path=str(BINARY), target_path=agent.binary)
        self.assertIn("env -i", agent.exec_as_root.call_args.args[1])
        self.assertNotIn("npm", agent.exec_as_root.call_args.args[1])


@unittest.skipUnless(BINARY.is_file(), "Install locked private V2 cache to run native setup tests")
class NativeSetupTests(unittest.TestCase):
    def test_actual_version_and_help(self):
        with tempfile.TemporaryDirectory(prefix="opencode-v2-test-") as root:
            env = v2.build_environment(request(), root, "synthetic-not-a-key")
            for args, expected in ((["--version"], "opencode v2.0.32"),
                                   (["run", "--help"], "--standalone")):
                proc = subprocess.run([str(BINARY), *args], env=env, cwd=root,
                                      text=True, capture_output=True, timeout=30)
                self.assertEqual(proc.returncode, 0, proc.stderr)
                self.assertIn(expected, proc.stdout)

    def test_successful_native_worker_accounts_all_provider_calls(self):
        self._success_fixture()

    def test_foreground_child_is_accounted_not_just_root_jsonl(self):
        self._success_fixture(spawn_child=True)

    def test_builtin_title_summary_compaction_selectors(self):
        for agent in ("title", "summary", "compaction"):
            with self.subTest(agent=agent):
                self._success_fixture(selected_agent=agent)

    def _success_fixture(self, spawn_child=False, selected_agent=None):
        bodies = []
        spawned = []
        events = [
            {"type": "message_start", "message": {"id": "msg_fixture", "type": "message", "role": "assistant",
                "model": v2.MODEL, "content": [], "stop_reason": None, "stop_sequence": None,
                "usage": {"input_tokens": 11, "output_tokens": 0, "cache_read_input_tokens": 3,
                          "cache_creation_input_tokens": 5}}},
            {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}},
            {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Offline fixture complete"}},
            {"type": "content_block_stop", "index": 0},
            {"type": "message_delta", "delta": {"stop_reason": "end_turn", "stop_sequence": None}, "usage": {"output_tokens": 7}},
            {"type": "message_stop"},
        ]
        data = b"".join(("event: " + event["type"] + "\ndata: " + json.dumps(event) + "\n\n").encode() for event in events)

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, format, *args):
                pass

            def do_POST(self):
                bodies.append(json.loads(self.rfile.read(int(self.headers["Content-Length"]))))
                response_data = data
                body = bodies[-1]
                if spawn_child and not spawned and any(tool.get("name") == "subagent" for tool in body.get("tools", [])):
                    spawned.append(True)
                    child_events = json.loads(json.dumps(events))
                    child_events[1]["content_block"] = {"type": "tool_use", "id": "toolu_offline_child", "name": "subagent", "input": {}}
                    child_events[2]["delta"] = {"type": "input_json_delta", "partial_json": json.dumps({
                        "agent": "general", "description": "Offline child accounting", "prompt": "child-fixture-marker", "background": False})}
                    child_events[4]["delta"]["stop_reason"] = "tool_use"
                    response_data = b"".join(("event: " + event["type"] + "\ndata: " + json.dumps(event) + "\n\n").encode() for event in child_events)
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.send_header("Content-Length", str(len(response_data)))
                self.end_headers()
                self.wfile.write(response_data)

        server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            with tempfile.TemporaryDirectory(prefix="opencode-v2-success-") as root:
                credential = Path(root) / "dummy.json"
                credential.write_text(json.dumps({"type": "api", "key": "offline-dummy-key"}))
                credential.chmod(0o600)
                req = request(binary=str(BINARY), directory=root, credential_path=str(credential),
                              max_output_tokens=32000, log_dir=str(Path(root) / "logs"),
                              result_path=str(Path(root) / "result.json"),
                              api_base_url=f"http://127.0.0.1:{server.server_port}/v1")
                req_path = Path(root) / "request.json"
                req_path.write_text(json.dumps(req))
                if selected_agent:
                    from unittest.mock import patch
                    utils_spec = importlib.util.spec_from_file_location("runtime_utils", PUBLIC / "neoism_harbor/runtime/runtime_utils.py")
                    assert utils_spec and utils_spec.loader
                    utils = importlib.util.module_from_spec(utils_spec)
                    utils_spec.loader.exec_module(utils)
                    real_command = v2.build_command

                    def helper_command(value):
                        command = real_command(value)
                        command[command.index("--agent") + 1] = selected_agent
                        return command

                    with patch.dict(sys.modules, {"runtime_utils": utils}), patch.object(v2, "build_command", helper_command):
                        helper_result = v2.run(req, {"type": "api", "key": "offline-dummy-key"})
                    Path(req["result_path"]).write_text(json.dumps(helper_result))
                    from types import SimpleNamespace
                    proc = SimpleNamespace(returncode=0 if helper_result["status"] == "completed" else 78, stderr="")
                else:
                    proc = subprocess.run([sys.executable, str(PUBLIC / "neoism_harbor/runtime/opencode_v2.py"),
                                           "--request", str(req_path)], cwd=root,
                                          env={"PATH": os.defpath, "HOME": root, "NEOISM_PUBLIC_EVAL_MOCK": "1"},
                                          text=True, capture_output=True, timeout=45)
                result = json.loads(Path(req["result_path"]).read_text())
                diagnostics = json.dumps(result) + proc.stderr
                if result.get("stdout_path"):
                    diagnostics += Path(result["stdout_path"]).read_text()[-8000:]
                for path in Path(req["log_dir"]).glob("*.log"):
                    diagnostics += path.read_text()[-3000:]
                self.assertEqual(proc.returncode, 0, diagnostics)
                self.assertEqual(result["status"], "completed", diagnostics)
                self.assertTrue(result["accounting_complete"])
                self.assertEqual(result["observed_models"], [v2.MODEL])
                if not selected_agent:
                    self.assertGreaterEqual(len(bodies), 2)  # root + native title helper
                    self.assertTrue(any(not body.get("tools") for body in bodies))
                    self.assertTrue(any(body.get("tools") for body in bodies))
                else:
                    self.assertGreaterEqual(len(bodies), 1)
                for body in bodies:
                    v2.check_wire_request(body)
                if spawn_child:
                    self.assertTrue(spawned)
                    self.assertGreaterEqual(len(bodies), 4, diagnostics)
                    self.assertTrue(any(any(message.get("role") == "user" and any(
                        "child-fixture-marker" in part.get("text", "") for part in message.get("content", []) if isinstance(part, dict))
                        for message in body.get("messages", [])) for body in bodies), diagnostics)
                self.assertEqual(len(result["provider_calls"]), len(bodies))
                self.assertEqual(result["usage"]["input_total"], 19 * len(bodies))
                self.assertEqual(result["usage"]["output_total"], 7 * len(bodies))
                self.assertGreater(result["native_audit"]["event_count"], 0)
                self.assertNotIn("offline-dummy-key", json.dumps(result))
                self.assertTrue((Path(req["log_dir"]) / "opencode-v2-audit.json").is_file())
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=5)

    def test_actual_cli_serializes_adaptive_high_to_loopback(self):
        """Fixture rejects requests with HTTP 401: zero model execution, no SSE claims."""
        bodies = []
        paths = []

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, format, *args):
                pass

            def do_POST(self):
                paths.append(self.path)
                bodies.append(json.loads(self.rfile.read(int(self.headers["Content-Length"]))))
                data = b'{"type":"error","error":{"type":"authentication_error","message":"offline fixture rejects inference"}}'
                self.send_response(401)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

        server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            with tempfile.TemporaryDirectory(prefix="opencode-v2-test-") as root:
                req = request(binary=str(BINARY), directory=root,
                              api_base_url=f"http://127.0.0.1:{server.server_port}/v1")
                env = v2.build_environment(req, root, "synthetic-not-a-key")
                proc = subprocess.Popen(v2.build_command(req), env=env, cwd=root,
                                        text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                        start_new_session=True)
                try:
                    stdout, stderr = proc.communicate(timeout=45)
                finally:
                    # Also remove any private-server descendants on failed setup.
                    try:
                        os.killpg(proc.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    proc.communicate()
                self.assertTrue(bodies, "No loopback request; CLI output: " + stdout[-4000:] + stderr[-4000:])
                self.assertTrue(all(path == "/v1/messages?beta=true" for path in paths), paths)
                for body in bodies:
                    self.assertEqual(body["model"], v2.MODEL)
                    self.assertEqual(body["thinking"]["type"], "adaptive")
                    self.assertEqual(body["output_config"]["effort"], "high")
                    self.assertEqual(body["max_tokens"], 32000)
                    self.assertTrue(body["stream"])
                    self.assertNotIn("synthetic-not-a-key", json.dumps(body))
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=5)


if __name__ == "__main__":
    unittest.main()
