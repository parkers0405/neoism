"""In-container utilities. No credential or provider access happens on import."""

import json
import math
import os
from pathlib import Path
import signal
import subprocess
import threading
import time
from urllib.parse import urlparse

MODEL = "claude-sonnet-5-5"


def redact(text, secrets):
    for secret in secrets:
        if secret:
            for ascii_only in (True, False):
                text = text.replace(json.dumps(secret, ensure_ascii=ascii_only)[1:-1], "[REDACTED]")
            text = text.replace(secret, "[REDACTED]")
    return text


class StreamRedactor:
    def __init__(self, secrets):
        patterns = set()
        for secret in secrets:
            if secret:
                patterns.add(secret.encode())
                patterns.update(json.dumps(secret, ensure_ascii=value)[1:-1].encode() for value in (True, False))
        self.patterns = sorted(patterns, key=len, reverse=True)
        self.tail = max((len(pattern) for pattern in self.patterns), default=1) - 1
        self.pending = b""

    def feed(self, data, final=False):
        self.pending += data
        boundary = len(self.pending) if final else max(0, len(self.pending) - self.tail)
        position, output = 0, []
        while position < boundary:
            matches = [(self.pending.find(pattern, position), pattern) for pattern in self.patterns]
            matches = [(at, pattern) for at, pattern in matches if position <= at < boundary]
            if not matches:
                output.append(self.pending[position:boundary])
                position = boundary
                break
            at, pattern = min(matches, key=lambda match: (match[0], -len(match[1])))
            output.extend((self.pending[position:at], b"[REDACTED]"))
            position = at + len(pattern)
        self.pending = self.pending[position:]
        return b"".join(output)


def load_request(path):
    request = json.loads(Path(path).read_text())
    if request.get("model") != MODEL or request.get("provider") != "anthropic":
        raise ValueError("Public experiment requires the pinned direct Anthropic model")
    if request.get("effort") != "high" or request.get("thinking") != {"type": "adaptive"}:
        raise ValueError("Unexpected reasoning policy")
    endpoint = urlparse(request.get("api_base_url", "https://api.anthropic.com/v1"))
    mock = request.get("test_mode") is True and request.get("mock_nonce") == "offline-native-wire-test"
    official = endpoint.scheme == "https" and endpoint.netloc == "api.anthropic.com" and endpoint.path.rstrip("/") == "/v1"
    fixture = mock and endpoint.scheme == "http" and endpoint.hostname in ("127.0.0.1", "localhost", "::1") and endpoint.path.rstrip("/") == "/v1"
    if not (official or fixture) or endpoint.username or endpoint.password or endpoint.query or endpoint.fragment:
        raise ValueError("Only the direct model endpoint is allowed")
    if not mock and request.get("timeout_seconds") != 3000:
        raise ValueError("The public task deadline is fixed at 3000 seconds")
    if not Path(request["directory"]).is_dir():
        raise ValueError("Repository directory does not exist")
    credential = json.loads(Path(request["credential_path"]).read_text())
    if (not isinstance(credential, dict) or set(credential) != {"type", "key"}
            or credential.get("type") != "api" or not isinstance(credential.get("key"), str)
            or not credential["key"].strip()):
        raise ValueError("A minimal Anthropic API-key projection is required")
    if mock and credential["key"] != "offline-dummy-key":
        raise ValueError("Mock mode accepts only the fixed dummy credential")
    return request, credential


def write_result(path, value):
    destination = Path(path)
    destination.parent.mkdir(parents=True, exist_ok=True)
    temporary = destination.with_name(destination.name + ".tmp")
    with temporary.open("w") as output:
        json.dump(value, output, indent=2, sort_keys=True, allow_nan=False)
        output.write("\n")
        output.flush()
        os.fsync(output.fileno())
    os.replace(temporary, destination)


def run_logged(command, env, cwd, timeout_seconds, log_dir, secrets):
    """Bounded sanitized streams and process-group cleanup, without shell execution."""
    directory = Path(log_dir)
    directory.mkdir(parents=True, exist_ok=True)
    stdout_path, stderr_path = directory / "stdout.jsonl", directory / "stderr.log"
    limited, errors = threading.Event(), []

    def capture(stream, path):
        redactor = StreamRedactor(secrets)
        remaining = output_remaining = 64 * 1024 * 1024
        try:
            with path.open("wb") as destination:
                while True:
                    data = stream.read1(65536)
                    if not data:
                        if remaining and output_remaining:
                            destination.write(redactor.feed(b"", final=True)[:output_remaining])
                        break
                    accepted = data[:remaining]
                    remaining -= len(accepted)
                    if output_remaining:
                        sanitized = redactor.feed(accepted)
                        destination.write(sanitized[:output_remaining])
                        output_remaining -= min(output_remaining, len(sanitized))
                        destination.flush()
                    if not remaining or not output_remaining:
                        limited.set()
        except Exception as error:
            errors.append(type(error).__name__)
            limited.set()

    started = time.monotonic()
    process = subprocess.Popen(command, env=env, cwd=cwd, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, stdin=subprocess.DEVNULL, start_new_session=True)
    readers = [threading.Thread(target=capture, args=pair, daemon=True) for pair in
               ((process.stdout, stdout_path), (process.stderr, stderr_path))]
    for reader in readers:
        reader.start()
    timed_out = False
    try:
        while process.poll() is None:
            if limited.is_set():
                break
            if time.monotonic() - started >= timeout_seconds:
                timed_out = True
                break
            time.sleep(0.05)
    finally:
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait(timeout=5)
        # A native daemon may outlive its leader; close the entire owned group.
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        for reader in readers:
            reader.join(timeout=5)
        for stream in (process.stdout, process.stderr):
            stream.close()
    if errors:
        raise RuntimeError("Agent log capture failed: " + ", ".join(errors))
    return {"returncode": process.returncode, "timed_out": timed_out,
            "log_limited": limited.is_set(), "duration_seconds": time.monotonic() - started,
            "stdout_path": str(stdout_path), "stderr_path": str(stderr_path)}


def worker_main(path, worker):
    """Admission failures also produce a sanitized, normalized result artifact.

    load_request checks the original endpoint BEFORE a private relay is created.
    Worker supervisors own post-launch failures and preserve their relay ledger.
    """
    from anthropic_gateway import apply_accounting
    request = {}
    launched = False
    try:
        loaded = json.loads(Path(path).read_text())
        if not isinstance(loaded, dict):
            raise ValueError("Request must be an object")
        request = loaded
        request, credential = load_request(path)
        launched = True
        return worker(request, credential)
    except Exception:
        # Never serialize auth parsing errors or arbitrary exception messages.
        result = {"status": "error", "timed_out": False,
                  "error": "Worker admission or supervision failed; no complete audit"}
        apply_accounting(result, None, False)
        if launched:
            # A supervisor failing outside its finally cannot prove zero spend.
            result.update(inference_started=None, estimated_cost_usd=None,
                          reserved_cost_usd=None, audit_scope="supervisor failed; accounting unavailable")
        write_result(request.get("result_path", "/logs/agent/eval-result.json"), result)
        return result
