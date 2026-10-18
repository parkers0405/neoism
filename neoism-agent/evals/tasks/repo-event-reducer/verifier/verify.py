"""Trusted assertions; candidate code is reached only through RPC."""
import unittest
from candidate_client import call


def event(i="a", v=1, key="x", value=None, op="put"):
    row = {"id": i, "key": key, "version": v, "op": op}
    if op == "put":
        row["value"] = value
    return row


def empty():
    return {"entities": {}, "seen": []}


class Acceptance(unittest.TestCase):
    def batch(self, events, state=None):
        state = empty() if state is None else state
        reply = call("reducer", "reduce_events", [state, events])
        self.assertEqual(reply["args_after"], [state, events])
        return reply["result"]

    def test_empty(self):
        self.assertEqual(self.batch([]), {"state": empty(), "applied": [], "ignored": [], "error": None})

    def test_replay(self):
        r = self.batch([event(), event()])
        self.assertEqual((r["applied"], r["ignored"], r["state"]["seen"]), (["a"], ["a"], ["a"]))

    def test_seen_id_across_keys(self):
        r = self.batch([event(), event(key="other", v=9)])
        self.assertNotIn("other", r["state"]["entities"])

    def test_stale(self):
        r = self.batch([event(v=4, value="new"), event("b", 2, value="old")])
        self.assertEqual(r["state"]["entities"]["x"]["value"], "new")
        self.assertEqual(r["state"]["seen"], ["a", "b"])
        self.assertEqual(r["ignored"], ["b"])

    def test_equal_version(self):
        r = self.batch([event(value=1), event("b", value=2)])
        self.assertEqual(r["ignored"], ["b"])
        self.assertEqual(r["state"]["entities"]["x"]["value"], 1)

    def test_tombstone_absent(self):
        r = self.batch([event(op="delete")])
        self.assertEqual(r["state"]["entities"], {"x": {"version": 1, "value": None, "deleted": True}})

    def test_tombstone_barrier(self):
        r = self.batch([event(v=4, op="delete"), event("b", 3, value=1)])
        self.assertTrue(r["state"]["entities"]["x"]["deleted"])
        self.assertEqual(r["ignored"], ["b"])

    def test_resurrection(self):
        r = self.batch([event(op="delete"), event("b", 2, value={"a": []})])
        self.assertEqual(r["state"]["entities"]["x"], {"version": 2, "value": {"a": []}, "deleted": False})

    def test_input_order(self):
        r = self.batch([event("z", key="z"), event("a", key="a"), event("z", 9)])
        self.assertEqual(r["applied"], ["z", "a"])
        self.assertEqual(r["ignored"], ["z"])

    def test_atomic_invalid_tail(self):
        self.assertEqual(self.batch([event(), {}]), {"state": empty(), "applied": [], "ignored": [], "error": {"code": "invalid_event", "index": 1}})

    def test_invalid_variants(self):
        for bad in [event(v=True), event(v=-1), event(i=""), event(op="rename"), dict(event(), extra=1), None]:
            with self.subTest(bad=bad):
                self.assertEqual(self.batch([bad])["error"], {"code": "invalid_event", "index": 0})

    def test_invalid_container(self):
        self.assertEqual(self.batch({})["error"], {"code": "invalid_event", "index": None})

    def test_invalid_state_precedence(self):
        bad = {"entities": {}, "seen": ["a", "a"]}
        self.assertEqual(self.batch(None, bad), {"state": bad, "applied": [], "ignored": [], "error": {"code": "invalid_state", "index": None}})

    def test_persisted_barrier(self):
        state = {"entities": {"x": {"version": 8, "value": None, "deleted": True}}, "seen": ["old"]}
        r = self.batch([event("new", 7)], state)
        self.assertEqual(r["state"]["entities"], state["entities"])
        self.assertEqual(r["ignored"], ["new"])

    def test_detached_result(self):
        state = {"entities": {"x": {"version": 0, "value": {"list": []}, "deleted": False}}, "seen": []}
        args = [state, []]
        r = call("reducer", "reduce_events", args, mutations=[{"target": "result", "path": ["state", "entities", "x", "value", "list"], "op": "append", "value": 1}])
        self.assertEqual(r["snapshots"][0]["args"], args)


if __name__ == "__main__":
    unittest.main()
