"""Read-only usage comparison for a captured lab results.json snapshot."""

import argparse
import json
from pathlib import Path
import statistics


def normalize(item):
    native = item["arm"] == "neoism-sol"
    tokens, usage = item.get("tokens", {}), item.get("usage", {})
    # Native TokenUsage separates cached input and subtracts reasoning from output.
    # Anthropic output_tokens already includes thinking; do not add it twice.
    fresh = tokens.get("input", 0) if native else usage.get("input_tokens", 0)
    cache_read = tokens.get("cache_read", 0) if native else usage.get("cache_read_input_tokens", 0)
    cache_write = tokens.get("cache_write", 0) if native else usage.get("cache_creation_input_tokens", 0)
    output = tokens.get("output", 0) + tokens.get("reasoning", 0) if native else usage.get("output_tokens", 0)
    return {"arm": item["arm"], "task": item["task"], "repeat": item["repeat"],
            "passed": item["passed"], "seconds": item["duration_seconds"],
            "fresh_input": fresh, "cache_read": cache_read, "cache_write": cache_write,
            "input": fresh + cache_read + cache_write, "output_including_reasoning": output,
            "total_tokens": fresh + cache_read + cache_write + output,
            "reported_api_cost_usd": item.get("reported_cost_usd"),
            "accounting_complete": item.get("accounting_complete", False)}


def summarize(items):
    count = len(items)
    passed = sum(item["passed"] for item in items)
    result = {"attempts": count, "passed": passed,
              "incomplete_usage_attempts": sum(not item["accounting_complete"] for item in items)}
    for field in ("seconds", "fresh_input", "cache_read", "cache_write", "input",
                  "output_including_reasoning", "total_tokens"):
        result[field] = sum(item[field] for item in items)
    result["mean_seconds"] = result["seconds"] / count if count else None
    result["median_seconds"] = statistics.median(item["seconds"] for item in items) if items else None
    result["tokens_per_attempt"] = result["total_tokens"] / count if count else None
    result["tokens_per_success_including_failures"] = result["total_tokens"] / passed if passed else None
    result["cache_read_fraction"] = result["cache_read"] / result["input"] if result["input"] else None
    costs = [item["reported_api_cost_usd"] for item in items]
    result["api_cost_usd"] = sum(costs) if costs and all(cost is not None for cost in costs) else None
    return result


def analyze(path):
    records = [normalize(item) for item in json.loads(path.read_text())
               if item.get("arm") in ("neoism-sol", "claude-sonnet")]
    arms = ("neoism-sol", "claude-sonnet")
    available = {arm: {(item["task"], item["repeat"]) for item in records if item["arm"] == arm} for arm in arms}
    paired = available[arms[0]] & available[arms[1]]
    return {"completed": len(records),
            "caveat": "Cumulative provider-reported tokens, including repeated context/cache reads; not equal billed dollars or identical tokenizers.",
            "overall": {arm: summarize([item for item in records if item["arm"] == arm]) for arm in arms},
            "matched_task_attempts": {"pairs": len(paired),
                "by_arm": {arm: summarize([item for item in records if item["arm"] == arm
                            and (item["task"], item["repeat"]) in paired]) for arm in arms},
                "tasks": [{"task": task, "repeat": repeat} for task, repeat in sorted(paired)]}}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("snapshot", type=Path)
    args = parser.parse_args()
    print(json.dumps(analyze(args.snapshot), indent=2))
