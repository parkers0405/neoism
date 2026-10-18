"""Offline fixtures; opt-in wire tests accept dummy credentials and loopback only.

Run: .venv/bin/python -m unittest discover -s tests -p test_native_adapters.py
Wire: NEOISM_NATIVE_WIRE_TEST=1 NEOISM_WIRE_BINARY=/.../neoism-agent
      CLAUDE_WIRE_BINARY=/.../native-claude <same command>
No auth files are opened and no paid endpoint is used.
"""
import hashlib
import importlib.util
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import sys
import tempfile
import threading
import types
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]


def load_runtime(name):
    shared = ROOT / "neoism_harbor/runtime/runtime_utils.py"
    spec = importlib.util.spec_from_file_location("runtime_utils", shared)
    utils = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(utils)
    spec = importlib.util.spec_from_file_location("native_fixture_" + name,
        ROOT / "neoism_harbor/runtime" / (name + ".py"))
    module = importlib.util.module_from_spec(spec)
    with patch.dict(sys.modules, {"runtime_utils": utils}), patch.object(
            sys, "path", [str(ROOT / "neoism_harbor/runtime")] + sys.path):
        spec.loader.exec_module(module)
    return module


neoism, claude = load_runtime("neoism"), load_runtime("claude_code")


def request(directory):
    return {"directory": str(directory), "prompt": "Reply with OK. Do not use tools.",
            "model": neoism.MODEL, "provider": "anthropic", "effort": "high",
            "thinking": {"type": "adaptive"}, "timeout_seconds": 30,
            "log_dir": str(directory / "logs"), "result_path": str(directory / "result.json"),
            "budget": {"maximum_api_cost_usd": 100, "context_limit": 1000000,
                       "pricing": {"input": 1, "output": 5, "cache_read": .1,
                                   "cache_write": 1.25, "cache_write_1h": 2}}}


class NativeFixtures(unittest.TestCase):
    def test_routes_and_explicit_reasoning(self):
        config = neoism.native_config("http://127.0.0.1:1/v1")["agent"]
        self.assertEqual(config["smallModel"], "anthropic/" + neoism.MODEL)
        self.assertEqual(set(config["agent"]), set(neoism.AGENTS))
        for value in config["agent"].values():
            self.assertEqual(value["model"], "anthropic/" + neoism.MODEL)
            self.assertEqual(value["options"], neoism.REASONING)
            self.assertFalse(value["tools"]["webfetch"])
        self.assertEqual(config["provider"]["anthropic"]["models"][neoism.MODEL]["options"],
                         dict(neoism.REASONING, max_tokens=32000))

    def test_endpoint_and_model_fail_closed(self):
        req = request(Path("/tmp"))
        for runtime in (neoism, claude):
            for base in ("https://evil.example/v1", "http://127.0.0.1:9/v1"):
                with self.assertRaises(ValueError):
                    runtime.validate_request(dict(req, api_base_url=base))
            with self.assertRaises(ValueError):
                runtime.validate_request(dict(req, model="other-model"))
            with self.assertRaises(ValueError):
                runtime.validate_request(dict(req, timeout_seconds=3001))
            runtime.validate_request(dict(req, api_base_url="http://127.0.0.1:9/v1",
                test_mode=True, mock_nonce="offline-native-wire-test"))

    def test_no_inherited_auth_or_provider_routing(self):
        with patch.dict(os.environ, {"OPENAI_API_KEY": "not-a-real-key",
                "ANTHROPIC_API_KEY": "not-a-real-key", "HTTPS_PROXY": "https://bad.invalid",
                "CLAUDE_CODE_USE_BEDROCK": "1", "NEOISM_AGENT_AUTH_CONTENT": "not-real"}):
            env = neoism.isolated_env(Path("/private"))
            self.assertNotIn("ANTHROPIC_API_KEY", env)
            self.assertNotIn("NEOISM_AGENT_AUTH_CONTENT", env)
            env = claude.child_env(Path("/private"), "offline-dummy-key", "http://127.0.0.1:9/v1")
            self.assertEqual(env["ANTHROPIC_API_KEY"], "offline-dummy-key")
            for name in ("ANTHROPIC_MODEL", "ANTHROPIC_DEFAULT_OPUS_MODEL", "ANTHROPIC_DEFAULT_HAIKU_MODEL",
                         "ANTHROPIC_SMALL_FAST_MODEL", "CLAUDE_CODE_SUBAGENT_MODEL"):
                self.assertEqual(env[name], neoism.MODEL)
            for key in ("OPENAI_API_KEY", "HTTPS_PROXY", "CLAUDE_CODE_USE_BEDROCK", "NEOISM_AGENT_AUTH_CONTENT"):
                self.assertNotIn(key, env)

    def test_authoritative_family_completion(self):
        runtime = {"runningBackgroundTasks": [], "execution": {"finished": True, "activeSegments": {}}}
        self.assertTrue(neoism.family_finished(runtime))
        self.assertFalse(neoism.family_finished(dict(runtime, runningBackgroundTasks=[{}])))
        self.assertFalse(neoism.family_finished(dict(runtime, execution={"finished": True, "activeSegments": {"child": {}}})))
        self.assertFalse(neoism.family_finished(dict(runtime, execution=None)))
        with self.assertRaises(RuntimeError):
            neoism.family_finished({})

    def test_child_usage_drift(self):
        def message(model):
            return {"info": {"role": "assistant", "providerId": "anthropic", "modelId": model,
                             "tokens": {"input": 3, "output": 2, "cache": {"read": 5}}}}
        usage = neoism.audit_usage({"root": [message(neoism.MODEL)], "child": [message(neoism.MODEL)]})
        self.assertEqual(usage["tokens"]["input"], 6)
        self.assertEqual(usage["tokens"]["cache_read"], 10)
        with self.assertRaises(RuntimeError):
            neoism.audit_usage({"root": [message(neoism.MODEL)], "child": [message("wrong")]})

    def test_interactions_are_limited_to_session_family(self):
        api = neoism.NativeAPI(1, "/app", 9999999999)
        calls = []
        def respond(method, path, body=None):
            calls.append((method, path, body))
            if method == "GET":
                return [{"id": "ours", "sessionId": "child"}, {"id": "other", "sessionId": "unrelated"}]
        api.request = respond
        api.interactions({"root", "child"})
        replies = [call for call in calls if call[0] == "POST"]
        self.assertEqual(replies, [("POST", "/interactions/permissions/ours/reply", {"reply": "once"}),
                                  ("POST", "/interactions/questions/ours/reject", None)])

    def test_claude_partial_stream_preserves_usage_before_missing_result(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "partial.jsonl"
            path.write_text(json.dumps({"type": "assistant", "parent_tool_use_id": "child",
                "message": {"id": "msg_partial", "model": claude.MODEL,
                            "usage": {"input_tokens": 8, "output_tokens": 2}}}) + "\n")
            partial = {}
            with self.assertRaises(RuntimeError):
                claude.audit_stream(path, partial)
            self.assertEqual(partial["observed_models"], [claude.MODEL])
            self.assertEqual(partial["usage_records"][0]["usage"]["input_tokens"], 8)

    def test_claude_native_terminal_accounting(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "stream.jsonl"
            final = {"type": "result", "is_error": False, "duration_ms": 7000, "duration_api_ms": 5000,
                     "usage": {"input_tokens": 11}, "modelUsage": {claude.MODEL: {"inputTokens": 11}},
                     "total_cost_usd": .01, "num_turns": 2}
            path.write_text(json.dumps(final) + "\n")
            result = claude.audit_stream(path)
            self.assertEqual(result["usage"], final["usage"])
            self.assertEqual(result["duration_api_ms"], 5000)
            final["modelUsage"]["wrong-model"] = {}
            path.write_text(json.dumps(final) + "\n")
            with self.assertRaises(RuntimeError):
                claude.audit_stream(path)


class InstallationFixtures(unittest.IsolatedAsyncioTestCase):
    async def test_real_harbor_subclasses_and_frozen_uploads(self):
        from harbor.agents.installed.base import BaseInstalledAgent
        from neoism_harbor.neoism import NeoismSonnet
        from neoism_harbor.claude_code import ClaudeSonnet
        from unittest.mock import AsyncMock
        with tempfile.TemporaryDirectory() as directory:
            artifact = Path(directory) / "dummy-elf-fixture"
            artifact.write_bytes(b"\x7fELFoffline-fixture-not-executed")
            digest = hashlib.sha256(artifact.read_bytes()).hexdigest()
            for cls, version in ((NeoismSonnet, "neoism-agent 0.7.128"), (ClaudeSonnet, "2.1.292 (Claude Code)")):
                adapter = cls(logs_dir=Path(directory), binary_path=str(artifact), binary_sha256=digest)
                self.assertIsInstance(adapter, BaseInstalledAgent)
                environment = types.SimpleNamespace(upload_file=AsyncMock())
                adapter.exec_as_root = AsyncMock(return_value=types.SimpleNamespace(return_code=0, stdout=version))
                await adapter.install_cli(environment)
                self.assertEqual(await adapter.runtime_binary(), adapter.binary)
                self.assertEqual(environment.upload_file.call_args.kwargs["target_path"], adapter.binary)
                self.assertIn("env -i", adapter.exec_as_root.call_args.kwargs["command"])
                adapter.exec_as_root = AsyncMock(return_value=types.SimpleNamespace(return_code=1, stdout=""))
                with self.assertRaises(RuntimeError):
                    await adapter.install_cli(environment)
                adapter.native_binary_sha256 = "0" * 64
                with self.assertRaises(ValueError):
                    await adapter.install_cli(environment)

    async def test_parent_bundle_wrapper_kept_separate_from_native_payload(self):
        from neoism_harbor.neoism import NeoismSonnet
        from unittest.mock import AsyncMock
        with tempfile.TemporaryDirectory() as directory:
            artifact = Path(directory) / "dummy-elf-fixture"
            artifact.write_bytes(b"\x7fELFoffline-fixture-not-executed")
            bundle = Path(directory) / "parent-runtime.tar"
            bundle.write_bytes(b"archive-fixture-not-executed")
            adapter = NeoismSonnet(logs_dir=Path(directory), binary_path=str(artifact), runtime_bundle=str(bundle))
            adapter.exec_as_root = AsyncMock(return_value=types.SimpleNamespace(return_code=0, stdout="neoism-agent 0.7.128"))
            environment = types.SimpleNamespace(upload_file=AsyncMock())
            (Path(directory) / "assets.lock.json").write_text(json.dumps({"runtime_bundle": {
                "path": str(bundle), "sha256": hashlib.sha256(bundle.read_bytes()).hexdigest()}}))
            with patch("neoism_harbor.common.ROOT", Path(directory)), patch("assets.validate_archive") as validate:
                await adapter.install_cli(environment)
                validate.assert_called_once_with(str(bundle))
            uploads = [call.kwargs["target_path"] for call in environment.upload_file.call_args_list]
            self.assertEqual(uploads, ["/opt/neoism-public-eval/runtime-bundle.tar", adapter.binary + ".real"])
            self.assertIn("tar -xf", adapter.exec_as_root.call_args_list[1].kwargs["command"])


class WireServer:
    def __init__(self, spawn_child=False):
        self.spawn_child = spawn_child
        self.child_issued = False

    def __enter__(self):
        self.requests = []
        owner = self
        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass
            def do_GET(self):
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.end_headers()
                self.wfile.write(b'{}')
            def do_POST(self):
                body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))) or b'{}')
                owner.requests.append((self.path, body))
                if "/messages" not in self.path or "count_tokens" in self.path:
                    payload = b'{"input_tokens":1}'
                    self.send_response(200)
                    self.send_header("Content-Type", "application/json")
                else:
                    # A deterministic SSE stop; absolutely no external inference.
                    events = [
                        {"type": "message_start", "message": {"id": "msg_fixture", "type": "message", "role": "assistant",
                         "model": neoism.MODEL, "content": [], "stop_reason": None, "stop_sequence": None,
                         "usage": {"input_tokens": 10, "output_tokens": 0}}},
                        {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}},
                        {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "OK"}},
                        {"type": "content_block_stop", "index": 0},
                        {"type": "message_delta", "delta": {"stop_reason": "end_turn", "stop_sequence": None}, "usage": {"output_tokens": 1}},
                        {"type": "message_stop"}]
                    tools = {tool.get("name") for tool in body.get("tools", [])}
                    selected = "task" if "task" in tools else ("Agent" if "Agent" in tools else None)
                    if owner.spawn_child and not owner.child_issued and selected:
                        owner.child_issued = True
                        arguments = {"description": "Offline child route fixture", "prompt": "Return OK without tools.",
                                     "subagent_type": "explore" if selected == "task" else "Explore"}
                        if selected == "Agent":
                            arguments["model"] = "haiku"  # Must resolve to the pinned default Haiku route.
                        events[1] = {"type": "content_block_start", "index": 0, "content_block": {
                            "type": "tool_use", "id": "tool_fixture_child", "name": selected, "input": {}}}
                        events[2] = {"type": "content_block_delta", "index": 0, "delta": {
                            "type": "input_json_delta", "partial_json": json.dumps(arguments)}}
                        events[4]["delta"]["stop_reason"] = "tool_use"
                    payload = "".join("event: " + event["type"] + "\ndata: " + json.dumps(event) + "\n\n" for event in events).encode()
                    self.send_response(200)
                    self.send_header("Content-Type", "text/event-stream")
                self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.base = "http://127.0.0.1:" + str(self.server.server_port) + "/v1"
        return self

    def __exit__(self, *args):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()


@unittest.skipUnless(os.environ.get("NEOISM_NATIVE_WIRE_TEST") == "1", "Explicit offline wire-test opt-in required")
class NativeWireFixtures(unittest.TestCase):
    def check_wire(self, runtime, binary, spawn_child=False):
        if not binary:
            self.skipTest("Frozen native artifact path not supplied")
        with tempfile.TemporaryDirectory(prefix="native-wire-fixture-") as private, WireServer(spawn_child=spawn_child) as wire:
            req = dict(request(Path(private)), binary=binary, api_base_url=wire.base,
                       test_mode=True, mock_nonce="offline-native-wire-test")
            result = runtime.run(req, {"type": "api", "key": "offline-dummy-key"})
            bodies = [body for path, body in wire.requests if "/messages" in path and "count_tokens" not in path]
            self.assertTrue(bodies, json.dumps(result))
            for body in bodies:
                self.assertEqual(body["model"], neoism.MODEL)
                self.assertEqual(body.get("thinking", {}).get("type"), "adaptive")
                self.assertLessEqual(body["max_tokens"], 32000)
                self.assertEqual(body.get("output_config", {}).get("effort"), "high")
            self.assertEqual(result["status"], "completed", json.dumps(result))
            self.assertNotIn("offline-dummy-key", Path(req["result_path"]).read_text())
            if spawn_child:
                self.assertTrue(wire.child_issued, "Native task/Agent tool not exposed: " + repr([tool.get("name") for tool in bodies[0].get("tools", [])]))
                self.assertGreaterEqual(len(bodies), 3, json.dumps(result))
                if runtime is neoism:
                    self.assertGreaterEqual(len(result["sessions"]), 2, json.dumps(result))
                else:
                    self.assertTrue(any(record.get("parent_tool_use_id") for record in result["usage_records"]), json.dumps(result))

    def test_neoism_07128_wire(self):
        self.check_wire(neoism, os.environ.get("NEOISM_WIRE_BINARY"))

    def test_claude_21292_wire(self):
        self.check_wire(claude, os.environ.get("CLAUDE_WIRE_BINARY"))

    def test_neoism_child_route_wire(self):
        self.check_wire(neoism, os.environ.get("NEOISM_WIRE_BINARY"), spawn_child=True)

    def test_claude_haiku_alias_child_route_wire(self):
        self.check_wire(claude, os.environ.get("CLAUDE_WIRE_BINARY"), spawn_child=True)


if __name__ == "__main__":
    unittest.main()
