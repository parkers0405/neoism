"""In-container adapters. No API request is made in preflight mode."""

import argparse
import base64
import hashlib
import json
import math
import os
from pathlib import Path
import selectors
import shutil
import signal
import stat
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

BASE = "http://127.0.0.1:14096/v2"
SECRETS = [os.environ[key] for key in ("OPENAI_API_KEY", "ANTHROPIC_API_KEY")
           if os.environ.get(key)]


def emit(value):
    text = json.dumps(value, ensure_ascii=True)
    for secret in SECRETS:
        for ascii_only in (True, False):
            text = text.replace(json.dumps(secret, ensure_ascii=ascii_only)[1:-1], "[REDACTED]")
    print(text, flush=True)


def request(method, path, body=None, timeout=10):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(BASE + path, data=data, method=method,
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as response:
        data = response.read(8 * 1024 * 1024 + 1)
        if len(data) > 8 * 1024 * 1024:
            raise RuntimeError("API response exceeds 8 MiB")
        return json.loads(data) if data else None


def messages(session, result=None):
    result = [] if result is None else result
    cursor, seen, collected = None, set(), []
    while True:
        query = {"limit": 256, "order": "asc"}
        if cursor:
            query["cursor"] = cursor
        page = request("GET", "/sessions/" + session + "/messages?" +
                       urllib.parse.urlencode(query))
        if (not isinstance(page, dict) or not isinstance(page.get("items"), list)
                or not isinstance(page.get("cursor"), dict)):
            raise RuntimeError("Unexpected transcript page")
        collected.extend(page["items"])
        if len(collected) >= len(result):
            result[:] = collected
        cursor = page["cursor"].get("next")
        if cursor is not None and not isinstance(cursor, str):
            raise RuntimeError("Unexpected transcript cursor")
        if not cursor:
            return result
        if cursor in seen:
            raise RuntimeError("Repeated transcript cursor")
        seen.add(cursor)


def children(session):
    page = request("GET", "/sessions/" + session + "/children")
    if (not isinstance(page, dict) or not isinstance(page.get("items"), list)
            or not isinstance(page.get("cursor"), dict)):
        raise RuntimeError("Unexpected child-session page")
    # This endpoint currently has no pagination query support. Never silently
    # omit descendants if the server starts returning a continuation.
    if page["cursor"].get("next") is not None:
        raise RuntimeError("Child-session pagination is unsupported")
    if any(not isinstance(child, dict) or not isinstance(child.get("id"), str)
           or not child["id"] for child in page["items"]):
        raise RuntimeError("Unexpected child-session item")
    return page["items"]


def collect_sessions(root, result=None):
    # Mutate the caller's snapshot incrementally so failed requests cannot erase
    # transcripts already collected (including child usage).
    result = {} if result is None else result
    pending, seen, errors = [root] + list(result), set(), []
    while pending:
        session = pending.pop()
        if session in seen:
            continue
        if len(seen) >= 64:
            raise RuntimeError("Subagent collection exceeded 64 sessions")
        seen.add(session)
        try:
            messages(session, result.setdefault(session, []))
        except Exception as error:
            errors.append(session + " transcript: " + str(error))
        try:
            pending.extend(child["id"] for child in children(session))
        except Exception as error:
            errors.append(session + " children: " + str(error))
    if errors:
        raise RuntimeError("; ".join(errors))
    return result


def service_interactions(family):
    for kind in ("permissions", "questions"):
        pending = request("GET", "/interactions/" + kind)
        if not isinstance(pending, list):
            raise RuntimeError("Invalid pending-interaction response")
        for item in pending:
            if item.get("sessionId") not in family:
                continue
            identifier = urllib.parse.quote(item["id"], safe="")
            path = "/interactions/" + kind + "/" + identifier
            if kind == "permissions":
                request("POST", path + "/reply", {"reply": "once"})
            else:
                request("POST", path + "/reject")
            emit({"type": "interaction_policy", "kind": kind, "session_id": item["sessionId"],
                  "permission": item.get("permission"), "patterns": item.get("patterns"),
                  "decision": "allow_once" if kind == "permissions" else "reject_question"})


def family_finished(runtime):
    if not isinstance(runtime, dict) or not isinstance(runtime.get("runningBackgroundTasks"), list):
        raise RuntimeError("Missing authoritative family runtime snapshot")
    execution = runtime.get("execution")
    if execution is None:
        return False
    if (not isinstance(execution, dict) or type(execution.get("finished")) is not bool
            or not isinstance(execution.get("activeSegments"), dict)):
        raise RuntimeError("Invalid family execution snapshot")
    return execution["finished"] and not runtime["runningBackgroundTasks"] and not execution["activeSegments"]


def usage(sessions):
    cost, assistants, errors, models, tokens = 0.0, 0, [], set(), {}
    complete_cost = True
    for session_messages in sessions.values():
        for message in session_messages:
            info = message["info"]
            if info.get("role") != "assistant":
                continue
            assistants += 1
            if info.get("error"):
                errors.append(info["error"])
            if info.get("modelId"):
                models.add(info["modelId"])
            value = info.get("cost")
            if not isinstance(value, bool) and isinstance(value, (int, float)) and math.isfinite(value) and value >= 0:
                cost += value
            else:
                complete_cost = False
            for key in ("input", "output", "reasoning"):
                tokens[key] = tokens.get(key, 0) + info.get("tokens", {}).get(key, 0)
            for key in ("read", "write"):
                label = "cache_" + key
                tokens[label] = tokens.get(label, 0) + info.get("tokens", {}).get("cache", {}).get(key, 0)
    return {"reported_cost_usd": cost if complete_cost and assistants else None,
            "assistant_messages": assistants, "errors": errors,
            "observed_models": sorted(models), "tokens": tokens,
            "usage_metered": (assistants > 0 and all(type(tokens.get(key)) is int and tokens[key] >= 0
                                                    for key in ("input", "output"))
                              and tokens["input"] + tokens["output"] > 0)}


def provider_environment(provider, credential):
    env = dict(os.environ)
    for key in ("OPENAI_API_KEY", "NEOISM_AGENT_OPENAI_API_KEY", "ANTHROPIC_API_KEY",
                "NEOISM_AGENT_AUTH_CONTENT", "NEOISM_AGENT_AUTH_PATH"):
        env.pop(key, None)
    if credential is not None:
        if provider == "openai":
            env["NEOISM_AGENT_AUTH_CONTENT"] = json.dumps({"openai": credential})
        else:
            env["ANTHROPIC_API_KEY"] = credential["key"]
    return env


def start_server(credential=None):
    home = Path(os.environ["HOME"])
    config = home / "config"
    config.mkdir(parents=True, exist_ok=True)
    (config / "config.json").write_text(json.dumps({"agent": {
        "defaultAgent": "build", "permission": {"*": "allow", "external_directory": "allow"},
    }}))
    env = dict(provider_environment("openai", credential), NEOISM_AGENT_STATE_DIR=str(home / "state"),
               NEOISM_AGENT_CONFIG_DIR=str(config))
    process = subprocess.Popen(["neoism-agent", "serve", "--hostname", "127.0.0.1",
                                "--port", "14096"], env=env,
                               stdout=sys.stderr, stderr=sys.stderr)
    deadline = time.monotonic() + 45
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError("Neoism server exited during readiness")
        try:
            request("GET", "/health", timeout=1)
            return process
        except (OSError, urllib.error.URLError):
            time.sleep(0.2)
    process.terminate()
    process.wait(timeout=5)
    raise RuntimeError("Neoism server readiness timed out")


def preflight():
    versions = {}
    for binary in ("neoism-agent", "claude", "python3", "git"):
        versions[binary] = subprocess.check_output([binary, "--version"], text=True).strip()
    process = start_server()
    try:
        session = request("POST", "/sessions?directory=%2Fworkspace", {"title": "offline-preflight"})
        # Creating an empty session and reading its transcript never submits a prompt.
        transcript = messages(session["id"])
        child_page = children(session["id"])
        family_finished(request("GET", "/sessions/" + session["id"] + "/runtime"))
        service_interactions({session["id"]})
        if transcript or child_page:
            raise RuntimeError("New preflight session is not empty")
        emit({"type": "preflight", "versions": versions,
              "worker_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              "verifier_sha256": hashlib.sha256(Path(__file__).with_name("verify.py").read_bytes()).hexdigest(),
              "candidate_hashes": {name: hashlib.sha256(Path(__file__).with_name(name).read_bytes()).hexdigest()
                                   for name in ("candidate_client.py", "candidate_worker.py")},
              "native_server_ready": True, "session_api_ready": True,
              "child_session_api_ready": True, "family_completion_api_ready": True,
              "empty_transcript": not transcript, "paid_requests": 0})
    finally:
        process.terminate()
        process.wait(timeout=5)


def initialize_repository():
    commands = [
        ["git", "-c", "init.templateDir=", "init", "-q", "-b", "main"],
        ["git", "config", "core.hooksPath", "/dev/null"],
        ["git", "add", "."],
        ["git", "-c", "user.name=Eval Lab", "-c", "user.email=eval@localhost",
         "commit", "-qm", "Task starting state"],
    ]
    env = dict(os.environ, GIT_AUTHOR_DATE="2026-01-01T00:00:00+00:00",
               GIT_COMMITTER_DATE="2026-01-01T00:00:00+00:00")
    for command in commands:
        subprocess.run(command, env=env, check=True, stdout=sys.stderr, stderr=sys.stderr)


def run_neoism(args, prompt):
    credential = getattr(args, "credential", None)
    subscription = credential is not None and credential.get("type") == "oauth"
    process, session = start_server(credential), None
    sessions, adapter_errors = {}, []
    reason = None
    try:
        session = request("POST", "/sessions?directory=%2Fworkspace", {
            "title": "eval", "permission": [
                {"permission": "*", "pattern": "*", "action": "allow"},
                {"permission": "external_directory", "pattern": "*", "action": "allow"}],
        })["id"]
        request("POST", "/sessions/" + session + "/prompt", {
            "prompt": prompt, "model": {"providerId": "openai", "modelId": args.model},
            "agent": "build", "variant": args.effort, "tools": {"question": False},
        })
        deadline = time.monotonic() + args.timeout
        while True:
            try:
                if time.monotonic() >= deadline:
                    reason = "timeout"
                snapshot = collect_sessions(session, sessions)
                current = usage(snapshot)
                if current["reported_cost_usd"] is not None and current["reported_cost_usd"] >= args.budget:
                    reason = "budget_exceeded"
                if not reason:
                    service_interactions(set(sessions) | {session})
                runtime = request("GET", "/sessions/" + session + "/runtime")
                if reason or family_finished(runtime):
                    emit({"type": "neoism_final_runtime", "runtime": runtime})
                if not reason and family_finished(runtime):
                    collect_sessions(session, sessions)
                    break
                if not reason and current["errors"]:
                    reason = "agent_error"
            except Exception as poll_error:
                reason = "adapter_error"
                adapter_errors.append("accounting/completion poll: " + str(poll_error))
            if reason:
                for active_session in set(sessions) | {session}:
                    try:
                        request("POST", "/sessions/" + active_session + "/abort", timeout=2)
                    except Exception as abort_error:
                        adapter_errors.append("abort: " + str(abort_error))
                break
            time.sleep(0.25)
        collect_sessions(session, sessions)
    except Exception as error:
        reason = "adapter_error"
        adapter_errors.append(str(error))
        if session:
            for active_session in set(sessions) | {session}:
                try:
                    request("POST", "/sessions/" + active_session + "/abort", timeout=2)
                except Exception as abort_error:
                    adapter_errors.append("abort: " + str(abort_error))
            try:
                collect_sessions(session, sessions)
            except Exception as collect_error:
                adapter_errors.append("final collection: " + str(collect_error))
    finally:
        try:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
        except Exception as cleanup_error:
            adapter_errors.append("server cleanup: " + str(cleanup_error))
            reason = "adapter_error"
        for sid, transcript in sessions.items():
            emit({"type": "neoism_transcript", "session_id": sid, "messages": transcript})
        try:
            summary = usage(sessions)
        except Exception as usage_error:
            adapter_errors.append("usage: " + str(usage_error))
            summary = {"reported_cost_usd": None}
            reason = "adapter_error"
        cost = summary["reported_cost_usd"]
        if subscription:
            if not summary.get("usage_metered"):
                reason = reason or "unmetered_usage"
        elif cost is None or cost <= 0:
            reason = reason or "unmetered_usage"
        if summary.get("errors") or not summary.get("assistant_messages"):
            reason = reason or "agent_error"
        mismatch = bool(summary.get("observed_models")) and summary["observed_models"] != [args.model]
        if mismatch:
            reason = reason or "model_mismatch"
        complete = (reason is None and not adapter_errors and
                    all(message["info"].get("time", {}).get("completed") is not None
                        for transcript in sessions.values() for message in transcript
                        if message["info"].get("role") == "assistant"))
        emit(dict(summary, type="lab_result", status=reason or "completed",
                  session_id=session, adapter_errors=adapter_errors,
                  model_mismatch=mismatch, accounting_complete=complete))


def run_claude(args, prompt):
    command = ["claude", "--print", "--verbose", "--output-format", "stream-json",
               "--model", args.model, "--effort", args.effort,
               "--max-budget-usd", str(args.budget), "--dangerously-skip-permissions",
               "--setting-sources", "", "--strict-mcp-config", "--mcp-config", '{"mcpServers":{}}',
               "--no-session-persistence", "--no-chrome", "--prompt-suggestions", "false"]
    # The container has a new HOME and no personal hooks, memory, plugins, or auth files.
    deadline = time.monotonic() + args.timeout
    process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                               stderr=sys.stderr, start_new_session=True,
                               env=provider_environment("anthropic", getattr(args, "credential", None)))
    assert process.stdin is not None and process.stdout is not None
    result, models, errors = None, set(), []
    status, buffer = "completed", bytearray()

    def consume(line):
        nonlocal result
        try:
            event = json.loads(line)
        except (json.JSONDecodeError, UnicodeDecodeError):
            emit({"type": "claude_non_json", "text": line.decode("utf-8", "replace")})
            return
        if not isinstance(event, dict):
            raise RuntimeError("Unexpected Claude event shape")
        emit({"type": "claude_event", "event": event})
        if event.get("type") == "assistant" and event.get("message", {}).get("model"):
            models.add(event["message"]["model"])
        if event.get("type") == "result":
            result = event

    try:
        # Multiplex prompt writes too: a CLI that never reads stdin must still
        # respect the same deadline as one that never finishes a stdout line.
        with selectors.DefaultSelector() as selector:
            os.set_blocking(process.stdin.fileno(), False)
            os.set_blocking(process.stdout.fileno(), False)
            pending = memoryview(prompt.encode())
            if pending:
                selector.register(process.stdin, selectors.EVENT_WRITE)
            else:
                process.stdin.close()
            selector.register(process.stdout, selectors.EVENT_READ)
            while selector.get_map():
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    status = "timeout"
                    break
                for key, _ in selector.select(min(remaining, 0.2)):
                    if key.fileobj is process.stdin:
                        written = os.write(process.stdin.fileno(), pending[:65536])
                        pending = pending[written:]
                        if not pending:
                            selector.unregister(process.stdin)
                            process.stdin.close()
                    else:
                        chunk = os.read(process.stdout.fileno(), 65536)
                        if not chunk:
                            selector.unregister(process.stdout)
                            if buffer:
                                consume(bytes(buffer))
                                buffer.clear()
                            continue
                        buffer.extend(chunk)
                        while b"\n" in buffer:
                            line, _, rest = buffer.partition(b"\n")
                            if len(line) > 2 * 1024 * 1024:
                                raise RuntimeError("Claude stream line exceeds 2 MiB")
                            buffer = bytearray(rest)
                            consume(bytes(line))
                        if len(buffer) > 2 * 1024 * 1024:
                            raise RuntimeError("Claude stream line exceeds 2 MiB")
            if status != "timeout":
                try:
                    process.wait(timeout=max(0, deadline - time.monotonic()))
                except subprocess.TimeoutExpired:
                    status = "timeout"
    except Exception as error:
        status = "adapter_error"
        errors.append(str(error))
    finally:
        # Kill the session's process group even if the leader already exited:
        # descendants may still own stdout or modify the workspace.
        try:
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                process.wait(timeout=1)
            except subprocess.TimeoutExpired:
                pass
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.wait(timeout=5)
        except Exception as error:
            errors.append("Claude cleanup: " + str(error))
            if status != "timeout":
                status = "adapter_error"
        process.stdout.close()
        if not process.stdin.closed:
            process.stdin.close()
    returncode = process.returncode
    if status == "completed" and (returncode or result is None or result.get("is_error")
                                  or result.get("subtype") != "success"):
        status = "agent_error"
    if result and isinstance(result.get("modelUsage"), dict):
        models.update(result["modelUsage"])
    mismatch = bool(models) and sorted(models) != [args.model]
    if mismatch:
        status = "model_mismatch" if status == "completed" else status
    cost = result.get("total_cost_usd") if result else None
    if (isinstance(cost, bool) or not isinstance(cost, (int, float))
            or not math.isfinite(cost) or cost < 0):
        cost = None
    if cost is None or cost == 0:
        status = "unmetered_usage" if status == "completed" else status
    emit({"type": "lab_result", "status": status, "reported_cost_usd": cost,
          "observed_models": sorted(models), "usage": result.get("usage") if result else None,
           "returncode": returncode, "adapter_errors": errors,
           "model_mismatch": mismatch, "accounting_complete": status == "completed" and not errors})


def export_workspace():
    """Export only bounded regular files beneath the fixed container workspace."""
    files, total, entries = {}, 0, 0
    directory_flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW

    def walk(directory, prefix="", depth=0):
        nonlocal total, entries
        if depth > 128:
            raise RuntimeError("Workspace directory nesting exceeds 128")
        with os.scandir(directory) as iterator:
            for entry in iterator:
                if entry.name in (".git", "__pycache__"):
                    continue
                entries += 1
                if entries > 10000:
                    raise RuntimeError("Workspace entry limit exceeded")
                relative = prefix + entry.name
                metadata = entry.stat(follow_symlinks=False)
                if stat.S_ISDIR(metadata.st_mode):
                    child = os.open(entry.name, directory_flags, dir_fd=directory)
                    try:
                        walk(child, relative + "/", depth + 1)
                    finally:
                        os.close(child)
                elif stat.S_ISREG(metadata.st_mode):
                    if len(files) >= 500:
                        raise RuntimeError("Workspace exceeds 500 files")
                    if metadata.st_size > 2 * 1024 * 1024:
                        raise RuntimeError("Workspace file exceeds 2 MiB: " + relative)
                    fd = os.open(entry.name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK,
                                 dir_fd=directory)
                    try:
                        opened = os.fstat(fd)
                        if (not stat.S_ISREG(opened.st_mode) or opened.st_nlink != 1
                                or (opened.st_dev, opened.st_ino) != (metadata.st_dev, metadata.st_ino)):
                            raise RuntimeError("Unsafe or changed workspace file: " + relative)
                        data = bytearray()
                        while True:
                            chunk = os.read(fd, min(65536, 2 * 1024 * 1024 + 1 - len(data)))
                            if not chunk:
                                break
                            data.extend(chunk)
                            if len(data) > 2 * 1024 * 1024:
                                raise RuntimeError("Workspace file exceeds 2 MiB: " + relative)
                            if total + len(data) > 8 * 1024 * 1024:
                                raise RuntimeError("Workspace exceeds 8 MiB raw")
                        total += len(data)
                        files[relative] = base64.b64encode(data).decode("ascii")
                    finally:
                        os.close(fd)
                else:
                    raise RuntimeError("Workspace contains symlink or special file: " + relative)

    try:
        root = os.open("/workspace", directory_flags)
        try:
            walk(root)
        finally:
            os.close(root)
        emit({"type": "lab_workspace", "files": files})
    except Exception as error:
        emit({"type": "lab_workspace_error", "error": str(error)})


def parse_trial_input(raw, mode, timeout):
    payload = json.loads(raw)
    if (not isinstance(payload, dict) or set(payload) != {"prompt", "credential"}
            or not isinstance(payload["prompt"], str) or not payload["prompt"].strip()):
        raise ValueError("Invalid private trial input")
    credential = payload["credential"]
    if credential is None:
        return payload["prompt"], None
    if not isinstance(credential, dict):
        raise ValueError("Invalid trial credential")
    kind = credential.get("type")
    if kind == "api":
        if set(credential) != {"type", "key"} or not isinstance(credential["key"], str) or not credential["key"]:
            raise ValueError("Invalid API credential")
    elif kind == "oauth" and mode == "neoism":
        if (not set(credential) <= {"type", "access", "refresh", "expires", "accountId"}
                or credential.get("refresh") != "" or not isinstance(credential.get("access"), str)
                or not credential["access"] or type(credential.get("expires")) is not int
                or credential["expires"] <= time.time_ns() // 1_000_000 + (timeout + 120) * 1000
                or ("accountId" in credential and not isinstance(credential["accountId"], str))):
            raise ValueError("OAuth access credential is invalid or expires too soon; refresh it in Neoism")
    else:
        raise ValueError("Unsupported trial credential type")
    return payload["prompt"], credential


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("mode", choices=["preflight", "neoism", "claude", "export"])
    parser.add_argument("--model")
    parser.add_argument("--effort", default="high")
    parser.add_argument("--budget", type=float)
    parser.add_argument("--timeout", type=int, default=180)
    args = parser.parse_args()
    if args.mode == "export":
        export_workspace()
        return
    Path(os.environ["HOME"]).mkdir(parents=True, exist_ok=True)
    if args.mode == "preflight":
        preflight()
    else:
        if not args.model or not args.budget or not math.isfinite(args.budget) or args.budget <= 0:
            parser.error("model and positive finite budget required")
        if args.timeout <= 0:
            parser.error("positive timeout required")
        try:
            raw = sys.stdin.read(2 * 1024 * 1024 + 1)
            if len(raw) > 2 * 1024 * 1024:
                raise RuntimeError("Trial input exceeds 2 MiB characters")
            prompt, args.credential = parse_trial_input(raw, args.mode, args.timeout)
            if args.credential:
                SECRETS.extend(args.credential[field] for field in ("access", "key") if args.credential.get(field))
            # Preserve seed symlinks without following them into unrelated paths;
            # export rejects any remaining symlinks rather than dereferencing.
            shutil.copytree('/seed', '/workspace', dirs_exist_ok=True, symlinks=True)
            os.chdir('/workspace')
            initialize_repository()
            if args.mode == "neoism":
                run_neoism(args, prompt)
            else:
                run_claude(args, prompt)
        finally:
            export_workspace()


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        emit({"type": "lab_result", "status": "adapter_error", "error": str(error),
              "reported_cost_usd": None})
        sys.exit(1)
