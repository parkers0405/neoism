"""Pure unit test source for shared relay accounting. Not executed this session.

No model calls, fixture servers, credential files or native binaries are needed.
"""
import importlib.util
import io
import json
from pathlib import Path
import unittest
from unittest.mock import Mock, patch

PATH = Path(__file__).resolve().parents[1] / "neoism_harbor/runtime/anthropic_gateway.py"
SPEC = importlib.util.spec_from_file_location("public_gateway_budget_fixture", PATH)
gateway = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(gateway)


def request(**updates):
    result = {"api_base_url": "https://api.anthropic.com/v1", "timeout_seconds": 3000,
              "budget": {"maximum_api_cost_usd": 10, "context_limit": 1000000,
                         "pricing": {"input": 1, "output": 5, "cache_read": .1,
                                     "cache_write": 1.25, "cache_write_1h": 2}}}
    result.update(updates)
    return result


def create(req=None):
    # Construction only: prevent even local bind. No thread is started.
    with patch.object(gateway, "ThreadingHTTPServer"):
        return gateway.AnthropicGateway(req or request(), "offline-dummy-key")


def feed(audit, event):
    audit.line(b"data: " + json.dumps(event).encode() + b"\n")
    audit.line(b"\n")


def start(audit, **usage):
    feed(audit, {"type": "message_start", "message": {
        "model": gateway.MODEL, "usage": dict(input_tokens=11, output_tokens=0, **usage)}})


class GatewayBudgetUnitTests(unittest.TestCase):
    def test_native_count_tokens_route_is_audited_but_not_inference(self):
        for model in (gateway.MODEL, "wrong-model"):
            with self.subTest(model=model), patch.object(gateway, "ThreadingHTTPServer") as server:
                relay = gateway.AnthropicGateway(request(), "offline-dummy-key")
                handler_class = server.call_args.args[1]
                handler = object.__new__(handler_class)
                payload = json.dumps({"model": model, "messages": []}).encode()
                handler.path = "/v1/messages/count_tokens?beta=true"
                handler.headers = {"Content-Length": str(len(payload)), "x-api-key": relay.native_key}
                handler.rfile = io.BytesIO(payload)
                handler.wfile = io.BytesIO()
                handler.send_response = Mock()
                handler.send_header = Mock()
                handler.end_headers = Mock()
                response = Mock(status=200)
                response.read.return_value = b'{"input_tokens":12}'
                with patch.object(gateway.http.client, "HTTPSConnection") as connection:
                    connection.return_value.getresponse.return_value = response
                    handler.do_POST()
                    if model == gateway.MODEL:
                        connection.return_value.request.assert_called_once()
                        self.assertTrue(relay.metadata_calls[0]["complete"])
                        self.assertEqual(handler.wfile.getvalue(), b'{"input_tokens":12}')
                    else:
                        connection.assert_not_called()
                        self.assertTrue(relay.poisoned)
                self.assertEqual(relay.calls, [])
                self.assertFalse(relay.accounting()["inference_started"])
                self.assertEqual(relay.accounting()["reserved_cost_usd"], 0)

    def test_normalized_runner_schema_keeps_raw_provider_usage(self):
        relay = create()
        relay.calls = [{"complete": True, "forwarded": True, "error": None, "usage": gateway.normalize_usage({
            "input_tokens": 2, "output_tokens": 7, "cache_read_input_tokens": 3,
            "cache_creation_input_tokens": 5})}]
        result = relay.accounting()
        self.assertEqual(result["tokens"], {"input_uncached": 2, "cache_read": 3, "cache_write": 5,
                                            "input_total": 10, "output_total": 7})
        self.assertEqual(result["provider_usage"]["cache_creation_input_tokens"], 5)
        self.assertTrue(result["accounting_complete"])
        failed = gateway.apply_accounting({}, None)
        self.assertEqual(set(failed["tokens"]), set(result["tokens"]))
        self.assertFalse(failed["accounting_complete"])

    def test_full_context_and_one_hour_surcharge_reserved(self):
        relay = create()
        call = {}
        reserved = relay.reserve_request(32000, call)
        self.assertEqual(float(reserved), 2.16)
        self.assertEqual(call["reserved_cost_usd"], 2.16)
        self.assertEqual(float(relay.reserved), 2.16)
        self.assertFalse(relay.accounting()["inference_started"])

    def test_concurrent_reservations_cannot_oversubscribe(self):
        req = request()
        req["budget"]["maximum_api_cost_usd"] = 3
        relay = create(req)
        relay.reserve_request(32000, {})
        with self.assertRaisesRegex(ValueError, "budget_exhausted"):
            relay.reserve_request(32000, {})
        self.assertEqual(float(relay.reserved), 2.16)

    def test_partial_accounting_blocks_later_inference(self):
        relay = create()
        relay.reserve_request(32000, {})
        relay.poisoned = True
        with self.assertRaisesRegex(ValueError, "partial_accounting_blocks_inference"):
            relay.reserve_request(1, {})
        self.assertEqual(float(relay.reserved), 2.16)

    def test_absolute_deadline_is_not_a_new_per_call_timeout(self):
        relay = create()
        self.assertLess(relay.deadline, gateway.time.monotonic() + 2960)
        relay.deadline = 0
        with self.assertRaisesRegex(ValueError, "absolute_deadline"):
            relay.reserve_request(1, {})
        self.assertEqual(float(relay.reserved), 0)

    def test_usage_ttl_and_cumulative_output(self):
        audit = gateway.SSEAudit()
        start(audit, cache_read_input_tokens=3, cache_creation_input_tokens=7,
              cache_creation={"ephemeral_5m_input_tokens": 2, "ephemeral_1h_input_tokens": 5})
        feed(audit, {"type": "message_delta", "usage": {"output_tokens": 4}})
        feed(audit, {"type": "message_delta", "usage": {"output_tokens": 9}})
        feed(audit, {"type": "message_stop"})
        usage = audit.finish()
        self.assertEqual(usage["output_tokens"], 9)
        self.assertEqual(usage["cache_creation_1h_input_tokens"], 5)
        self.assertAlmostEqual(float(create().cost(usage)), .0000688)

    def test_missing_ttl_is_priced_conservatively(self):
        usage = gateway.normalize_usage({"input_tokens": 0, "output_tokens": 0,
                                         "cache_creation_input_tokens": 10})
        self.assertEqual(float(create().cost(usage)), .00002)

    def test_inconsistent_ttl_and_incomplete_sse_are_not_authoritative(self):
        with self.assertRaises(ValueError):
            gateway.normalize_usage({"input_tokens": 1, "output_tokens": 0,
                "cache_creation_input_tokens": 3,
                "cache_creation": {"ephemeral_5m_input_tokens": 1, "ephemeral_1h_input_tokens": 1}})
        audit = gateway.SSEAudit()
        start(audit)
        feed(audit, {"type": "message_delta", "usage": {"output_tokens": 3}})
        with self.assertRaisesRegex(ValueError, "incomplete_usage"):
            audit.finish()

    def test_exact_wire_policy(self):
        body = {"model": gateway.MODEL, "thinking": {"type": "adaptive"},
                "output_config": {"effort": "high"}, "stream": True, "max_tokens": 32000}
        gateway.check_wire_request(body)
        for key, value in (("model", "other"), ("stream", False), ("max_tokens", 32001),
                           ("thinking", {"type": "enabled"}), ("output_config", {"effort": "low"})):
            with self.subTest(key=key), self.assertRaises(ValueError):
                gateway.check_wire_request(dict(body, **{key: value}))

    def test_endpoint_mock_and_opaque_auth(self):
        with self.assertRaises(ValueError):
            create(request(api_base_url="http://127.0.0.1:9/v1"))
        relay = create(request(api_base_url="http://127.0.0.1:9/v1", test_mode=True,
                               mock_nonce="offline-native-wire-test"))
        self.assertNotEqual(relay.native_key, "offline-dummy-key")
        self.assertNotIn("offline-dummy-key", json.dumps(relay.accounting()))

    def test_background_tool_evidence_survives_shared_parser(self):
        audit = gateway.SSEAudit()
        start(audit)
        feed(audit, {"type": "content_block_start", "index": 0,
                     "content_block": {"type": "tool_use", "name": "subagent", "input": {}}})
        feed(audit, {"type": "content_block_delta", "index": 0,
                     "delta": {"partial_json": '{"background":true}'}})
        feed(audit, {"type": "content_block_stop", "index": 0})
        self.assertTrue(audit.background_child_seen)


if __name__ == "__main__":
    unittest.main()
