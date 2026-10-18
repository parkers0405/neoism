"""No candidate imports in the trusted process."""
import unittest
from candidate_client import call


def row(key, deps=None, version=1, tx=None):
    return {"id": key, "requires": deps or [], "version": version, "transaction": tx}


def step(ids, tx=None):
    return {"transaction": tx, "migrations": ids}


class Acceptance(unittest.TestCase):
    def plan(self, rows, targets, installed=None):
        installed = {} if installed is None else installed
        args = [rows, installed, targets]
        response = call("planner", "plan", args)
        self.assertEqual(response["args_after"], args)
        return response["result"]

    def test_empty_preserves_versions(self):
        self.assertEqual(self.plan([], [], {"legacy": 7}), {"steps": [], "versions": {"legacy": 7}, "error": None})

    def test_dependency_closure(self):
        r = self.plan([row("a"), row("z", ["a"])], ["z"])
        self.assertEqual(r, {"steps": [step(["a"]), step(["z"])], "versions": {"a": 1, "z": 1}, "error": None})

    def test_reverse_lexical_dependency(self):
        r = self.plan([row("a", ["z"]), row("z")], ["a", "z"])
        self.assertEqual(r["steps"], [step(["z"]), step(["a"])])

    def test_ready_tiebreak(self):
        r = self.plan([row("z"), row("b"), row("a", ["b"])], ["z", "a"])
        self.assertEqual(r["steps"], [step(["b"]), step(["a"]), step(["z"])])

    def test_duplicate_targets(self):
        self.assertEqual(self.plan([row("a")], ["a", "a"])["steps"], [step(["a"])])

    def test_satisfied_skips_closure(self):
        self.assertEqual(self.plan([row("a", ["missing"])], ["a"], {"a": 4})["steps"], [])
        self.assertEqual(self.plan([row("a")], ["a"], {"a": 4})["versions"], {"a": 4})

    def test_version_upgrade_and_satisfied_dependency(self):
        r = self.plan([row("a", version=3), row("b", ["a"], version=2)], ["b"], {"a": 3, "b": 1})
        self.assertEqual(r["steps"], [step(["b"])])
        self.assertEqual(r["versions"], {"a": 3, "b": 2})

    def test_transaction_grouping(self):
        r = self.plan([row("a", tx="t"), row("z", ["a"], tx="t"), row("m")], ["z", "m"])
        self.assertEqual(r["steps"], [step(["a", "z"], "t"), step(["m"])])

    def test_transaction_internal_order(self):
        r = self.plan([row("a", ["z"], tx="t"), row("z", tx="t")], ["a", "z"])
        self.assertEqual(r["steps"], [step(["z", "a"], "t")])

    def test_external_dependency_before_group(self):
        r = self.plan([row("a", ["z"], tx="t"), row("b", tx="t"), row("z")], ["a", "b"])
        self.assertEqual(r["steps"], [step(["z"]), step(["a", "b"], "t")])

    def test_cycle_atomic(self):
        r = self.plan([row("a", ["b"]), row("b", ["a"])], ["a", "b"], {"legacy": 9})
        self.assertEqual(r, {"steps": [], "versions": {"legacy": 9}, "error": {"code": "cycle"}})

    def test_transaction_cycle(self):
        rows = [row("a", tx="t"), row("b", ["a"]), row("c", ["b"], tx="t")]
        self.assertEqual(self.plan(rows, ["a", "b", "c"])["error"], {"code": "transaction_cycle"})

    def test_missing_dependency_atomic(self):
        r = self.plan([row("a"), row("b", ["missing"])], ["a", "b"], {"missing": 9})
        self.assertEqual(r, {"steps": [], "versions": {"missing": 9}, "error": {"code": "missing_dependency"}})

    def test_unknown_target(self):
        self.assertEqual(self.plan([], ["absent"], {"absent": 8})["error"], {"code": "unknown_target"})

    def test_unreachable_graph_defects(self):
        rows = [row("a"), row("b", ["b"]), row("z", ["missing"])]
        self.assertEqual(self.plan(rows, ["a"])["steps"], [step(["a"])])

    def test_invalid_schema_precedence(self):
        variants = [[row("a"), row("a")], [row("a", version=True)], [row("a", ["x", "x"])], [dict(row("a"), extra=1)], None]
        for rows in variants:
            with self.subTest(rows=rows):
                self.assertEqual(self.plan(rows, ["missing"])["error"], {"code": "invalid_input"})
        self.assertEqual(self.plan([], [], {"a": -1})["error"], {"code": "invalid_input"})


if __name__ == "__main__":
    unittest.main()
