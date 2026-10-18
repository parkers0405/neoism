"""Transparent Anthropic relay and catalog-price admission bound (not an invoice).

Required request.budget schema (all prices USD per million tokens):
{maximum_api_cost_usd: positive number, context_limit: positive integer,
 pricing: {input: number, output: number, cache_read: number,
           cache_write: number, cache_write_1h: number}}
Prices must be frozen, finite, nonnegative, with input/output strictly positive.
cache_write_1h is the FULL price including the one-hour surcharge, not a delta.
Provider billing is external. No auth/config/network access occurs on import.
"""
import copy
from decimal import Decimal
import http.client
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import math
import socket
import secrets
import threading
import time
from urllib.parse import urlsplit

MODEL = "claude-sonnet-5-5"
TOKEN_FIELDS = ("input_tokens", "output_tokens", "cache_read_input_tokens",
                "cache_creation_input_tokens", "cache_creation_5m_input_tokens",
                "cache_creation_1h_input_tokens")


def number(value):
    if type(value) not in (int, float) or not math.isfinite(value) or value < 0:
        raise ValueError("invalid_budget")
    return Decimal(str(value))


def token_count(value):
    if type(value) is not int or value < 0:
        raise ValueError("unknown_usage")
    return value


def check_wire_request(body, cap=32000):
    if not isinstance(body, dict) or body.get("model") != MODEL:
        raise ValueError("request_model_mismatch")
    if body.get("thinking") != {"type": "adaptive"} or body.get("output_config", {}).get("effort") != "high":
        raise ValueError("request_reasoning_mismatch")
    value = body.get("max_tokens")
    if type(value) is not int or not 0 < value <= min(cap, 32000) or body.get("stream") is not True:
        raise ValueError("request_limits_mismatch")


def normalize_usage(usage):
    result = {key: token_count(usage[key]) for key in ("input_tokens", "output_tokens")}
    for key in ("cache_read_input_tokens", "cache_creation_input_tokens"):
        result[key] = token_count(usage.get(key, 0))
    cache = usage.get("cache_creation")
    if cache is None:
        # Older responses omit TTL details: conservatively price all writes at
        # the more expensive TTL. These are accounting buckets, not guessed TTLs.
        result["cache_creation_5m_input_tokens"] = 0
        result["cache_creation_1h_input_tokens"] = 0
    else:
        five = token_count(cache["ephemeral_5m_input_tokens"])
        hour = token_count(cache["ephemeral_1h_input_tokens"])
        if five + hour != result["cache_creation_input_tokens"]:
            raise ValueError("unknown_usage")
        result["cache_creation_5m_input_tokens"] = five
        result["cache_creation_1h_input_tokens"] = hour
    return result


class SSEAudit:
    """Cumulative Messages usage; retain no text, tool input, IDs or headers."""
    def __init__(self):
        self.started = self.delta = self.stopped = False
        self.model = None
        self.usage = {}
        self.data = []
        self.data_size = 0
        self.subagent_blocks = {}
        self.background_child_seen = False

    def line(self, line):
        if len(line) > 4 * 1024 * 1024:
            raise ValueError("oversized_sse_event")
        line = line.rstrip(b"\r\n")
        if line.startswith(b"data:"):
            self.data.append(line[5:].lstrip())
            self.data_size += len(line)
            if self.data_size > 4 * 1024 * 1024:
                raise ValueError("oversized_sse_event")
        if line or not self.data:
            return
        event = json.loads(b"\n".join(self.data))
        self.data, self.data_size = [], 0
        kind = event.get("type")
        if self.stopped:
            raise ValueError("unexpected_sse_after_stop")
        if kind == "error":
            raise ValueError("provider_error")
        if kind == "message_start":
            if self.started:
                raise ValueError("duplicate_message_start")
            message = event["message"]
            if message.get("model") != MODEL:
                raise ValueError("response_model_mismatch")
            self.model = MODEL
            self.usage = dict(message["usage"])
            normalize_usage(self.usage)
            self.started = True
        elif kind == "message_delta":
            if not self.started:
                raise ValueError("unexpected_message_delta")
            usage = event["usage"]
            if token_count(usage["output_tokens"]) < self.usage["output_tokens"]:
                raise ValueError("unknown_usage")
            # Counters are cumulative snapshots, never additive deltas.
            for key in ("input_tokens", "output_tokens", "cache_read_input_tokens", "cache_creation_input_tokens"):
                if key in usage and token_count(usage[key]) < token_count(self.usage.get(key, 0)):
                    raise ValueError("unknown_usage")
            self.usage.update(usage)
            normalize_usage(self.usage)
            self.delta = True
        elif kind == "content_block_start":
            block = event["content_block"]
            if block.get("type") == "tool_use" and block.get("name") in ("subagent", "Task", "Agent", "task", "agent"):
                self.subagent_blocks[event["index"]] = {"initial": block.get("input", {}), "json": "", "name": block["name"]}
        elif kind == "content_block_delta" and event.get("index") in self.subagent_blocks:
            block = self.subagent_blocks[event["index"]]
            block["json"] += event["delta"].get("partial_json", "")
            if len(block["json"]) > 4 * 1024 * 1024:
                raise ValueError("oversized_sse_event")
        elif kind == "content_block_stop" and event.get("index") in self.subagent_blocks:
            block = self.subagent_blocks.pop(event["index"])
            value = json.loads(block["json"]) if block["json"] else block["initial"]
            self.background_child_seen |= value.get("background") is True or value.get("run_in_background") is True
            if block["name"] == "subagent" and value.get("model") not in (None, "anthropic/" + MODEL, "anthropic/" + MODEL + "#high"):
                raise ValueError("request_model_mismatch")
        elif kind == "message_stop":
            if not self.started or not self.delta or self.subagent_blocks:
                raise ValueError("incomplete_usage")
            self.stopped = True

    def finish(self):
        if not self.started or not self.delta or not self.stopped or self.data or self.subagent_blocks:
            raise ValueError("incomplete_usage")
        return normalize_usage(self.usage)


class AnthropicGateway:
    def __init__(self, request, key, deadline=None):
        # Leave bounded native cancellation/accounting time inside Harbor's
        # equal outer deadline; the relay never grants a new per-call timeout.
        outer_deadline = deadline if deadline is not None else time.monotonic() + request["timeout_seconds"]
        self.deadline = outer_deadline - min(45.0, request["timeout_seconds"] / 10)
        self.result_path = request.get("result_path")
        endpoint = urlsplit(request.get("api_base_url", "https://api.anthropic.com/v1"))
        mock = request.get("test_mode") is True and request.get("mock_nonce") == "offline-native-wire-test"
        if request.get("test_mode") and (not mock or key != "offline-dummy-key"):
            raise ValueError("invalid_mock_auth")
        official = endpoint.scheme == "https" and endpoint.netloc == "api.anthropic.com" and endpoint.path.rstrip("/") == "/v1"
        fixture = mock and key == "offline-dummy-key" and endpoint.scheme == "http" and endpoint.hostname in ("127.0.0.1", "::1", "localhost") and endpoint.path.rstrip("/") == "/v1"
        if not (official or fixture) or endpoint.username or endpoint.password or endpoint.query or endpoint.fragment:
            raise ValueError("invalid_upstream")
        budget = request.get("budget", {})
        self.maximum = number(budget.get("maximum_api_cost_usd"))
        self.context = budget.get("context_limit")
        if self.maximum <= 0 or type(self.context) is not int or self.context <= 0:
            raise ValueError("invalid_budget")
        self.pricing = {k: number(budget.get("pricing", {}).get(k)) for k in ("input", "output", "cache_read", "cache_write", "cache_write_1h")}
        if (self.pricing["input"] <= 0 or self.pricing["output"] <= 0
                or self.pricing["cache_write_1h"] < max(self.pricing["cache_write"], 2 * self.pricing["input"])):
            raise ValueError("invalid_budget")
        self.cap = request.get("max_output_tokens", 32000)
        if type(self.cap) is not int or not 0 < self.cap <= 32000:
            raise ValueError("request_limits_mismatch")
        self.endpoint, self.key = endpoint, key
        # Native children cannot authenticate a mistakenly direct provider call.
        self.native_key = "relay-only-" + secrets.token_urlsafe(32)
        self.lock = threading.Lock()
        self.calls = []
        self.metadata_calls = []
        self.spent = self.reserved = Decimal(0)
        self.poisoned = self.closed = False
        self.connections = set()
        self.clients = set()
        gateway = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, format, *args):
                pass

            def setup(self):
                super().setup()
                self.connection.settimeout(max(.001, gateway.deadline - time.monotonic()))
                with gateway.lock:
                    gateway.clients.add(self.connection)
                    closed = gateway.closed
                if closed:
                    self.connection.close()

            def finish(self):
                try:
                    super().finish()
                finally:
                    with gateway.lock:
                        gateway.clients.discard(self.connection)

            def do_count_tokens(self):
                # The native CLIs use Anthropic's free token-count endpoint for
                # context management; this is not a Messages inference call.
                connection = None
                record = {"route": "count_tokens", "complete": False, "model": MODEL}
                with gateway.lock:
                    gateway.metadata_calls.append(record)
                try:
                    if self.headers.get("Transfer-Encoding") or self.headers.get("x-api-key") != gateway.native_key:
                        raise ValueError("unexpected_auth")
                    length = int(self.headers.get("Content-Length", "0"))
                    if not 0 < length <= 64 * 1024 * 1024:
                        raise ValueError("invalid_request_length")
                    payload = self.rfile.read(length)
                    body = json.loads(payload)
                    if len(payload) != length or not isinstance(body, dict) or body.get("model") != MODEL:
                        raise ValueError("request_model_mismatch")
                    with gateway.lock:
                        if gateway.closed or gateway.poisoned or time.monotonic() >= gateway.deadline:
                            raise ValueError("absolute_deadline")
                    cls = http.client.HTTPSConnection if gateway.endpoint.scheme == "https" else http.client.HTTPConnection
                    connection = cls(gateway.endpoint.hostname, gateway.endpoint.port, timeout=max(.001, gateway.deadline - time.monotonic()))
                    with gateway.lock:
                        gateway.connections.add(connection)
                    headers = {k: v for k, v in self.headers.items() if k.lower() not in
                               ("host", "connection", "transfer-encoding", "accept-encoding", "authorization", "x-api-key")}
                    headers.update({"x-api-key": gateway.key, "Accept-Encoding": "identity"})
                    connection.request("POST", self.path, payload, headers)
                    response = connection.getresponse()
                    value = response.read(1024 * 1024 + 1)
                    if len(value) > 1024 * 1024 or response.status != 200:
                        raise ValueError("invalid_token_count_response")
                    token_count(json.loads(value)["input_tokens"])
                    self.send_response(200)
                    self.send_header("Content-Type", "application/json")
                    self.send_header("Content-Length", str(len(value)))
                    self.send_header("Connection", "close")
                    self.end_headers()
                    self.wfile.write(value)
                    record["complete"] = True
                except Exception:
                    with gateway.lock:
                        record["error"] = "token_count_audit_failed"
                        gateway.poisoned = True
                    try:
                        self.send_response(502)
                        self.send_header("Content-Length", "0")
                        self.send_header("Connection", "close")
                        self.end_headers()
                    except OSError:
                        pass
                finally:
                    self.close_connection = True
                    if connection:
                        connection.close()
                        with gateway.lock:
                            gateway.connections.discard(connection)

            def do_POST(self):
                if self.path in ("/v1/messages/count_tokens", "/v1/messages/count_tokens?beta=true"):
                    self.do_count_tokens()
                    return
                call = {"complete": False, "request_checked": False, "forwarded": False,
                        "model": None, "usage": None, "error": "incomplete_call", "reserved_cost_usd": 0.0}
                with gateway.lock:
                    gateway.calls.append(call)
                connection, headers_sent = None, False
                try:
                    if self.path not in ("/v1/messages", "/v1/messages?beta=true"):
                        raise ValueError("unsupported_provider_route")
                    if self.headers.get("Transfer-Encoding"):
                        raise ValueError("invalid_request_length")
                    length = int(self.headers.get("Content-Length", "0"))
                    if not 0 < length <= 64 * 1024 * 1024:
                        raise ValueError("invalid_request_length")
                    payload = self.rfile.read(length)
                    if len(payload) != length:
                        raise ValueError("invalid_request_length")
                    body = json.loads(payload)
                    check_wire_request(body, gateway.cap)
                    if self.headers.get("x-api-key") != gateway.native_key:
                        raise ValueError("unexpected_auth")
                    call["request_checked"] = True
                    reserve = gateway.reserve_request(body["max_tokens"], call)
                    cls = http.client.HTTPSConnection if gateway.endpoint.scheme == "https" else http.client.HTTPConnection
                    connection = cls(gateway.endpoint.hostname, gateway.endpoint.port, timeout=max(.001, gateway.deadline - time.monotonic()))
                    with gateway.lock:
                        gateway.connections.add(connection)
                    connection.connect()
                    with gateway.lock:
                        if gateway.closed or gateway.poisoned or time.monotonic() >= gateway.deadline:
                            raise ValueError("absolute_deadline")
                        # Mark immediately before request I/O: a transport error
                        # cannot prove zero spend, even if connect succeeded.
                        call["forwarded"] = True
                    headers = {k: v for k, v in self.headers.items() if k.lower() not in
                               ("host", "connection", "transfer-encoding", "accept-encoding", "authorization", "x-api-key")}
                    headers.update({"x-api-key": gateway.key, "Accept-Encoding": "identity"})
                    connection.request("POST", self.path, payload, headers)
                    response = connection.getresponse()
                    self.send_response(response.status)
                    for name, value in response.getheaders():
                        if name.lower() not in ("connection", "transfer-encoding", "content-length", "keep-alive"):
                            self.send_header(name, value)
                    self.send_header("Connection", "close")
                    self.end_headers()
                    headers_sent = True
                    if response.status != 200 or "text/event-stream" not in response.getheader("Content-Type", ""):
                        # Preserve native provider errors unchanged, but do not
                        # infer zero billing or release their reservation.
                        while time.monotonic() < gateway.deadline:
                            chunk = response.read1(65536)
                            if not chunk:
                                break
                            self.wfile.write(chunk)
                            self.wfile.flush()
                        raise ValueError("non_success_sse_response")
                    audit = SSEAudit()
                    while True:
                        if time.monotonic() >= gateway.deadline:
                            raise ValueError("absolute_deadline")
                        line = response.readline(4 * 1024 * 1024 + 1)
                        if not line:
                            break
                        audit.line(line)
                        # Preserve the child's final lifecycle signal even if a
                        # later transport failure prevents authoritative usage.
                        with gateway.lock:
                            call["background_child_seen"] = audit.background_child_seen
                        self.wfile.write(line)
                        self.wfile.flush()
                    usage = audit.finish()
                    actual = gateway.cost(usage)
                    with gateway.lock:
                        if gateway.closed:
                            raise ValueError("absolute_deadline")
                        gateway.spent += actual
                        gateway.reserved -= reserve
                        call.update(complete=True, model=MODEL, usage=usage, error=None,
                                    estimated_cost_usd=float(actual), reserved_cost_usd=0.0)
                        if actual > reserve:
                            gateway.poisoned = True
                            call["error"] = "catalog_bound_exceeded"
                except Exception as error:
                    safe = str(error) if isinstance(error, ValueError) and str(error) in SAFE_ERRORS else "gateway_transport_or_schema_error"
                    with gateway.lock:
                        call["error"] = safe
                        # Never release a reservation after ambiguous transport.
                        # Even denied/malformed calls invalidate trial success.
                        gateway.poisoned = True
                    if not headers_sent:
                        try:
                            self.send_response(502)
                            self.send_header("Content-Length", "0")
                            self.send_header("Connection", "close")
                            self.end_headers()
                        except OSError:
                            pass
                finally:
                    self.close_connection = True
                    if connection:
                        connection.close()
                        with gateway.lock:
                            gateway.connections.discard(connection)

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.server.daemon_threads = True
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.timer = threading.Timer(max(0, self.deadline - time.monotonic()), self.expire)
        self.timer.daemon = True

    def reserve_request(self, output_cap, call):
        with self.lock:
            if self.closed or time.monotonic() >= self.deadline:
                raise ValueError("absolute_deadline")
            if self.poisoned:
                raise ValueError("partial_accounting_blocks_inference")
            reserve = (Decimal(self.context) * max(self.pricing[k] for k in ("input", "cache_read", "cache_write", "cache_write_1h")) + Decimal(output_cap) * self.pricing["output"]) / Decimal(1000000)
            if self.spent + self.reserved + reserve > self.maximum:
                raise ValueError("budget_exhausted")
            self.reserved += reserve
            call["reserved_cost_usd"] = float(reserve)
            return reserve

    def cost(self, usage):
        five, hour = usage["cache_creation_5m_input_tokens"], usage["cache_creation_1h_input_tokens"]
        unknown = usage["cache_creation_input_tokens"] - five - hour
        p = self.pricing
        return (Decimal(usage["input_tokens"]) * p["input"] + Decimal(usage["output_tokens"]) * p["output"] +
                Decimal(usage["cache_read_input_tokens"]) * p["cache_read"] + Decimal(five) * p["cache_write"] +
                Decimal(hour) * p["cache_write_1h"] + Decimal(unknown) * max(p["cache_write"], p["cache_write_1h"])) / Decimal(1000000)

    @property
    def url(self):
        return "http://127.0.0.1:" + str(self.server.server_port) + "/v1"

    def __enter__(self):
        self.thread.start()
        self.timer.start()
        return self

    def expire(self, checkpoint=True):
        with self.lock:
            self.closed = True
            connections = list(self.connections)
            clients = list(self.clients)
        for client in clients:
            try:
                client.shutdown(socket.SHUT_RDWR)
                client.close()
            except OSError:
                pass
        for connection in connections:
            try:
                if connection.sock:
                    connection.sock.shutdown(socket.SHUT_RDWR)
                connection.close()
            except OSError:
                pass

        if checkpoint and self.result_path:
            # Persist before native cancellation/transcript cleanup. The worker
            # replaces this with its richer final artifact when cleanup finishes.
            from runtime_utils import write_result
            value = self.accounting()
            value.update(status="timeout", timed_out=True, accounting_complete=False,
                         error="Absolute inference deadline; native cleanup pending")
            write_result(self.result_path, value)

    def __exit__(self, *args):
        self.timer.cancel()
        self.expire(checkpoint=False)
        # A deadline checkpoint must finish before the worker's final atomic
        # write, otherwise a late timer could overwrite native cleanup results.
        self.timer.join()
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=2)

    def snapshot(self):
        with self.lock:
            return copy.deepcopy(self.calls)

    def accounting(self):
        with self.lock:
            calls = copy.deepcopy(self.calls)
            totals = {key: sum(c["usage"].get(key, 0) for c in calls if c.get("complete") and c.get("usage")) for key in TOKEN_FIELDS}
            totals["input_total"] = totals["input_tokens"] + totals["cache_read_input_tokens"] + totals["cache_creation_input_tokens"]
            totals["output_total"] = totals["output_tokens"]
            tokens = {"input_uncached": totals["input_tokens"], "cache_read": totals["cache_read_input_tokens"],
                      "cache_write": totals["cache_creation_input_tokens"], "input_total": totals["input_total"],
                      "output_total": totals["output_total"]}
            return {"tokens": tokens, "provider_usage": totals,
                    "accounting_complete": bool(calls) and not self.poisoned and all(c["complete"] for c in calls),
                    "estimated_cost_usd": float(self.spent), "reserved_cost_usd": float(self.reserved),
                    "inference_started": any(c["forwarded"] for c in calls), "provider_calls": calls,
                    "metadata_calls": copy.deepcopy(self.metadata_calls),
                    "maximum_api_cost_usd": float(self.maximum), "budget_exhausted": any(c["error"] == "budget_exhausted" for c in calls),
                    "audit_scope": "all calls through isolated Anthropic relay: root, auxiliary, compaction, children; not a network sandbox",
                    "cost_basis": "frozen supplied catalog-price bound; actual provider billing external; unknown cache TTL priced at maximum write rate"}


SAFE_ERRORS = frozenset(("request_model_mismatch", "response_model_mismatch", "request_reasoning_mismatch",
    "request_limits_mismatch", "unknown_usage", "incomplete_usage", "provider_error", "unsupported_provider_route",
    "unexpected_auth", "invalid_request_length", "non_success_sse_response", "oversized_sse_event",
    "duplicate_message_start", "unexpected_message_delta", "unexpected_sse_after_stop", "absolute_deadline",
    "budget_exhausted", "partial_accounting_blocks_inference"))


def apply_accounting(result, gateway, lifecycle_complete=True):
    # Preserve native schemas before replacing their nonuniform token totals.
    if "tokens" in result:
        result["native_tokens"] = result["tokens"]
    if "accounting_complete" in result:
        result["native_accounting_complete"] = result["accounting_complete"]
    if "observed_models" in result:
        result["native_observed_models"] = result["observed_models"]
    if gateway is None:
        result.update(tokens={key: 0 for key in ("input_uncached", "cache_read", "cache_write", "input_total", "output_total")},
                      provider_usage=dict({key: 0 for key in TOKEN_FIELDS}, input_total=0, output_total=0),
                      accounting_complete=False,
                      estimated_cost_usd=0.0, reserved_cost_usd=0.0, inference_started=False,

                      audit_scope="relay admission failed before native launch")
    else:
        result.update(gateway.accounting())
    calls = result.get("provider_calls", [])
    result["observed_models"] = sorted({call["model"] for call in calls if call.get("model")})
    result["model_mismatch"] = bool(result.get("model_mismatch")) or any(
        call.get("error") in ("request_model_mismatch", "response_model_mismatch") for call in calls)
    result["lifecycle_complete"] = bool(lifecycle_complete)
    result["accounting_complete"] &= bool(lifecycle_complete)
    if not result["accounting_complete"] and result.get("status") == "completed":
        result["status"] = "error"
    return result
