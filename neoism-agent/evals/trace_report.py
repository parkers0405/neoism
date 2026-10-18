#!/usr/bin/env python3
"""Offline, allowlisted trace statistics. Never emits text, arguments or lab_workspace.

Assistant IDs count logical messages, not guaranteed HTTP requests. Provider-active
seconds are accounting segments, not pure inference or necessarily wall time.
"""
import argparse
import collections
import json
from pathlib import Path
import statistics


def stats(values):
    values = [v for v in values if isinstance(v, (int, float))]
    return {"n": len(values), "sum": sum(values), "mean": statistics.mean(values) if values else None,
            "median": statistics.median(values) if values else None}


def tool_summary(tools):
    grouped = collections.defaultdict(list)
    for t in tools:
        grouped[t["name"]].append(t)
    return {name: {"count": len(ts), "statuses": dict(collections.Counter(t["status"] for t in ts)),
                   "duration_seconds": stats([t.get("seconds") for t in ts])}
            for name, ts in sorted(grouped.items())}


def parse_trace(path, arm, root_session=None):
    messages, tools, sessions, parents = {}, {}, set(), set()
    runtime, cli = {}, {}
    nested = collections.Counter()
    # No traversal of lab_workspace or generic recursive extraction.
    with Path(path).open() as f:
        for line in f:
            row = json.loads(line)
            kind = row.get("type")
            if kind == "neoism_final_runtime":
                runtime = row.get("runtime", {})
                root_session = runtime.get("rootSessionId", root_session)
            elif kind == "neoism_transcript":
                sid = row.get("session_id")
                sessions.add(sid)
                for message in row.get("messages", []):
                    info = message.get("info", {})
                    if info.get("role") != "assistant":
                        continue
                    mid = info.get("id")
                    if mid:
                        messages[mid] = {"session": info.get("sessionId", sid), "time": info.get("time", {})}
                    for part in message.get("parts", []):
                        if part.get("type") != "tool":
                            continue
                        tid = part.get("callId") or part.get("id")
                        if not tid:
                            continue
                        state = part.get("state", {})
                        timing = state.get("time", {})
                        seconds = ((timing["end"] - timing["start"]) / 1000
                                   if all(isinstance(timing.get(k), (int, float)) for k in ("start", "end")) else None)
                        tools[tid] = {"name": part.get("tool", "unknown"), "session": info.get("sessionId", sid),
                                      "status": state.get("status", "unknown"), "seconds": seconds,
                                      "metadata": state.get("metadata", {})}
            elif isinstance(row.get("event"), dict):
                event = row["event"]
                parent = event.get("parent_tool_use_id")
                if parent:
                    parents.add(parent)
                if event.get("type") == "result":
                    cli = {k: event.get(k) for k in ("duration_ms", "duration_api_ms", "num_turns", "ttft_ms",
                           "ttft_stream_ms", "time_to_request_ms", "first_content_frame_ms")}
                    cli["model_usage"] = {model: {k: usage.get(k) for k in ("inputTokens", "outputTokens", "cacheReadInputTokens", "cacheCreationInputTokens", "thinkingTokens")}
                                          for model, usage in event.get("modelUsage", {}).items()}
                    cli["subagent_stats"] = {k: event.get("subagent_stats", {}).get(k) for k in ("spawned", "completed", "failed", "max_depth")}
                message = event.get("message", {})
                if event.get("type") == "assistant" and message.get("id"):
                    messages[message["id"]] = {"session": "child" if parent else "root"}
                    for part in message.get("content", []):
                        if part.get("type") == "tool_use" and part.get("id"):
                            tools[part["id"]] = {"name": part.get("name", "unknown"), "session": "child" if parent else "root", "status": "unknown", "seconds": None}
                if event.get("type") == "user":
                    for part in message.get("content", []) if isinstance(message.get("content"), list) else []:
                        if part.get("type") == "tool_result":
                            tid = part.get("tool_use_id")
                            if tid in tools:
                                tools[tid]["status"] = "error" if part.get("is_error") else "completed"
    root = root_session if arm == "neoism-sol" else "root"
    for tool in tools.values():
        # Only structured metadata is eligible; do not interpret code or Bash chains.
        if "compose" in tool["name"]:
            for op in tool.get("metadata", {}).get("operations", []):
                if isinstance(op, dict) and isinstance(op.get("tool"), str):
                    nested[op["tool"]] += 1
    task_metadata = collections.Counter()
    for tool in tools.values():
        if tool["name"].lower() in ("task", "agent"):
            md = tool.get("metadata", {})
            # Only known enums/booleans; do not copy arbitrary metadata strings.
            task_metadata["background_true" if md.get("background") is True else "background_other"] += 1
            if md.get("agent") == "explore":
                task_metadata["explore"] += 1
            if md.get("status") == "running":
                task_metadata["metadata_running_at_launch"] += 1
    execution = runtime.get("execution", {})
    activities = execution.get("sessionActivities", {})
    return {"assistant_messages": len(messages), "root_assistant_messages": sum(m["session"] == root for m in messages.values()),
            "child_assistant_messages": sum(m["session"] != root for m in messages.values()),
            "child_sessions": len(sessions - {root, None}) if arm == "neoism-sol" else None,
            "parent_tool_use_ids": len(parents), "tools": tool_summary(tools.values()),
            "root_tools": tool_summary(t for t in tools.values() if t["session"] == root),
            "child_tools": tool_summary(t for t in tools.values() if t["session"] != root),
            "compose_nested_operations": dict(nested), "task_metadata_counts": dict(task_metadata),
            "runtime_activity_sessions": len(activities),
            "runtime_activity_child_sessions": len(set(activities) - {root}),
            "provider_active_seconds": execution.get("completedMs", 0) / 1000 if "completedMs" in execution else None,
            "session_provider_active_seconds": sorted(v["completedMs"] / 1000 for v in execution.get("sessionActivities", {}).values() if "completedMs" in v),
            "cli": cli}


def summarize(rows):
    out: dict = {"attempts": len(rows), "passed": sum(r["passed"] for r in rows)}
    out["task_metadata_counts"] = dict(sum((collections.Counter(r["task_metadata_counts"]) for r in rows), collections.Counter()))
    for key in ("wall_seconds", "assistant_messages", "root_assistant_messages", "child_assistant_messages", "child_sessions", "parent_tool_use_ids", "provider_active_seconds"):
        out[key] = stats([r.get(key) for r in rows])
    for scope in ("tools", "root_tools", "child_tools"):
        names = sorted({name for r in rows for name in r[scope]})
        out[scope] = {}
        for name in names:
            ts = [r[scope][name] for r in rows if name in r[scope]]
            statuses = collections.Counter()
            for t in ts:
                statuses.update(t["statuses"])
            out[scope][name] = {"count": sum(t["count"] for t in ts), "statuses": dict(statuses),
                                "timed_count": sum(t["duration_seconds"]["n"] for t in ts),
                                "summed_seconds": sum(t["duration_seconds"]["sum"] for t in ts)}
    out["cli"] = {k: stats([r["cli"].get(k) for r in rows]) for k in ("duration_ms", "duration_api_ms", "num_turns", "ttft_ms", "ttft_stream_ms", "time_to_request_ms", "first_content_frame_ms")}
    return out


def report(run):
    rows = []
    for path in sorted(Path(run).glob("*--*/result.json")):
        result = json.loads(path.read_text())
        if result.get("arm") not in ("neoism-sol", "claude-sonnet"):
            continue
        row = {k: result[k] for k in ("task", "repeat", "arm", "passed")}
        row["wall_seconds"] = result["duration_seconds"]
        row.update(parse_trace(path.with_name("agent.jsonl"), row["arm"], result.get("session_id")))
        rows.append(row)
    grouped = collections.defaultdict(dict)
    for row in rows:
        grouped[(row["task"], row["repeat"])][row["arm"]] = row
    pairs = []
    for (task, repeat), arms in sorted(grouped.items()):
        if len(arms) != 2:
            continue
        n, c = arms["neoism-sol"], arms["claude-sonnet"]
        pairs.append({"task": task, "repeat": repeat, "both_passed": n["passed"] and c["passed"],
                      "native_seconds": n["wall_seconds"], "claude_seconds": c["wall_seconds"],
                      "ratio": n["wall_seconds"] / c["wall_seconds"], "excess_seconds": n["wall_seconds"] - c["wall_seconds"],
                      "native_messages": n["assistant_messages"], "claude_messages": c["assistant_messages"],
                      "message_ratio": n["assistant_messages"] / c["assistant_messages"] if c["assistant_messages"] else None})
    subsets = {}
    for name, selected in (("matched", pairs), ("common_passed", [p for p in pairs if p["both_passed"]])):
        keys = {(p["task"], p["repeat"]) for p in selected}
        subsets[name] = {arm: summarize([r for r in rows if r["arm"] == arm and (r["task"], r["repeat"]) in keys]) for arm in ("neoism-sol", "claude-sonnet")}
        subsets[name]["paired_ratios"] = stats([p["ratio"] for p in selected])
        subsets[name]["paired_message_ratios"] = stats([p["message_ratio"] for p in selected])
    return {"subsets": subsets, "pairs": pairs, "attempts": rows,
            "limitations": ["Logical assistant IDs are not measured HTTP request counts.",
                            "Native completedMs sums active provider segments, not pure inference or wall time; tool sums can overlap.",
                            "Runtime sessionActivities coverage is reported separately; do not assume child provider time is included in completedMs.",
                            "Claude CLI TTFT fields are recorded result-level scalars, not a per-request TTFT distribution; native comparable request/TTFT is unknown.",
                            "Claude tool elapsed times are unknown; Bash chains are not equivalent to native file-tool operations.",
                            "Cache rate and provider/model differences are confounded, not a causal cache-speed experiment."]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("run", type=Path)
    parser.add_argument("--output", type=Path, help="Optional NEW derived report (exclusive creation)")
    args = parser.parse_args()
    data = report(args.run)
    text = json.dumps(data, indent=2, sort_keys=True) + "\n"
    if args.output:
        with args.output.open("x") as f:
            f.write(text)
    else:
        print(text, end="")


if __name__ == "__main__":
    main()
