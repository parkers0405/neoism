import json
from pathlib import Path
import tempfile
import unittest

from trace_report import parse_trace


class TraceReportTests(unittest.TestCase):
    def parse(self, rows, arm):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "agent.jsonl"
            path.write_text("\n".join(json.dumps(row) for row in rows))
            return parse_trace(path, arm)

    def test_native_duplicate_snapshots_children_and_runtime(self):
        tool = {"type": "tool", "callId": "t", "tool": "read", "state": {"status": "completed", "input": {"secret": "NEVER_EMIT"}, "time": {"start": 1000, "end": 1250}}}
        transcript = {"type": "neoism_transcript", "session_id": "root", "messages": [{"info": {"role": "assistant", "id": "m", "sessionId": "root"}, "parts": [tool]}]}
        child = {"type": "neoism_transcript", "session_id": "child", "messages": [{"info": {"role": "assistant", "id": "child-m"}, "parts": []}]}
        report = self.parse([transcript, transcript, child, {"type": "neoism_final_runtime", "runtime": {"rootSessionId": "root", "execution": {"completedMs": 9000}}}, {"type": "lab_workspace", "files": {"secret": "NEVER_EMIT"}}], "neoism-sol")
        self.assertEqual(report["assistant_messages"], 2)
        self.assertEqual(report["child_sessions"], 1)
        self.assertEqual(report["tools"]["read"]["count"], 1)
        self.assertEqual(report["tools"]["read"]["duration_seconds"]["sum"], .25)
        self.assertEqual(report["provider_active_seconds"], 9)
        self.assertNotIn("NEVER_EMIT", json.dumps(report))

    def test_claude_repeated_stream_blocks_and_parent(self):
        def event(kind, message, parent=None):
            return {"type": "claude_event", "event": {"type": kind, "message": message, "parent_tool_use_id": parent}}
        assistant = event("assistant", {"id": "a", "content": [{"type": "tool_use", "id": "t", "name": "Bash", "input": {"command": "NEVER_EMIT"}}]})
        result = event("user", {"content": [{"type": "tool_result", "tool_use_id": "t", "is_error": True}]})
        report = self.parse([assistant, assistant, result, event("assistant", {"id": "b", "content": []}, "t")], "claude-sonnet")
        self.assertEqual(report["assistant_messages"], 2)
        self.assertEqual(report["root_assistant_messages"], 1)
        self.assertEqual(report["parent_tool_use_ids"], 1)
        self.assertEqual(report["tools"]["Bash"]["statuses"], {"error": 1})
        self.assertEqual(report["tools"]["Bash"]["duration_seconds"]["n"], 0)
        self.assertNotIn("NEVER_EMIT", json.dumps(report))


if __name__ == "__main__":
    unittest.main()
