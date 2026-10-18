"""Offline token-normalization contracts; never contacts either provider."""

import json
from pathlib import Path
import tempfile
import unittest

import usage_report


def result(arm, repeat=1, passed=True):
    return {"arm": arm, "task": "task", "repeat": repeat, "passed": passed,
            "duration_seconds": 10, "accounting_complete": True}


class UsageReportTests(unittest.TestCase):
    def test_native_cache_and_reasoning_added_once(self):
        item = result("neoism-sol")
        item["tokens"] = {"input": 100, "cache_read": 20, "cache_write": 10,
                          "output": 40, "reasoning": 5}
        normalized = usage_report.normalize(item)
        self.assertEqual(normalized["input"], 130)
        self.assertEqual(normalized["output_including_reasoning"], 45)
        self.assertEqual(normalized["total_tokens"], 175)

    def test_claude_thinking_not_added_twice(self):
        item = result("claude-sonnet")
        item["usage"] = {"input_tokens": 100, "cache_read_input_tokens": 20,
                         "cache_creation_input_tokens": 10, "output_tokens": 45,
                         "output_tokens_details": {"thinking_tokens": 5}}
        normalized = usage_report.normalize(item)
        self.assertEqual(normalized["input"], 130)
        self.assertEqual(normalized["output_including_reasoning"], 45)
        self.assertEqual(normalized["total_tokens"], 175)

    def test_failed_attempt_usage_counts_per_success(self):
        success, failure = result("claude-sonnet"), result("claude-sonnet", passed=False)
        success["usage"], failure["usage"] = {"output_tokens": 10}, {"output_tokens": 20}
        success["reported_cost_usd"], failure["reported_cost_usd"] = 0.1, 0.2
        summary = usage_report.summarize([usage_report.normalize(item) for item in (success, failure)])
        self.assertEqual(summary["tokens_per_attempt"], 15)
        self.assertEqual(summary["tokens_per_success_including_failures"], 30)
        self.assertAlmostEqual(summary["api_cost_usd"], 0.3)

    def test_matched_attempts_require_both_task_and_repeat(self):
        with tempfile.TemporaryDirectory() as directory:
            snapshot = Path(directory) / "results.json"
            snapshot.write_text(json.dumps([result("neoism-sol", 1), result("neoism-sol", 2),
                                            result("claude-sonnet", 2), {"arm": "oracle"}]))
            report = usage_report.analyze(snapshot)
        self.assertEqual(report["completed"], 3)
        self.assertEqual(report["matched_task_attempts"]["pairs"], 1)
        self.assertEqual(report["matched_task_attempts"]["tasks"], [{"task": "task", "repeat": 2}])
        self.assertIsNone(report["overall"]["neoism-sol"]["api_cost_usd"])


if __name__ == "__main__":
    unittest.main()
