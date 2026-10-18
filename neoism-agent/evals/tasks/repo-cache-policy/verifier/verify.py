import unittest
from candidate_client import call


def put(key, value=1, now=0, **extra):
    return dict({"op": "put", "now": now, "key": key, "value": value}, **extra)


def op(kind, key="a", now=0):
    return {"op": kind, "key": key, "now": now}


def resize(capacity, now=0):
    return {"op": "resize", "now": now, "capacity": capacity}


class Acceptance(unittest.TestCase):
    def sim(self, operations, capacity=2, ttl=None):
        args = [{"capacity": capacity, "default_ttl": ttl}, operations]
        r = call("cache_api", "simulate", args)
        self.assertEqual(r["args_after"], args)
        return r["result"]

    def test_empty(self):
        self.assertEqual(self.sim([]), {"entries": [], "capacity": 2, "results": [], "error": None})

    def test_lru_get_promotes(self):
        r = self.sim([put("a"), put("b"), op("get", "a"), put("c")])
        self.assertEqual([e["key"] for e in r["entries"]], ["a", "c"])
        self.assertEqual(r["results"][-1], {"op": "put", "expired": [], "stored": True, "evicted": ["b"]})

    def test_peek_does_not_promote(self):
        r = self.sim([put("a"), put("b"), op("peek", "a"), put("c")])
        self.assertEqual([e["key"] for e in r["entries"]], ["b", "c"])
        self.assertEqual(r["results"][-1]["evicted"], ["a"])

    def test_overwrite_promotes_and_resets_ttl(self):
        r = self.sim([put("a", now=0), put("b", now=1), put("a", value=9, now=2), op("peek", now=5)], ttl=4)
        self.assertEqual(r["entries"], [{"key": "a", "value": 9, "expires_at": 6}])
        self.assertEqual(r["results"][-1]["expired"], ["b"])

    def test_expiry_boundary(self):
        r = self.sim([put("a", now=3), op("get", now=7), op("get", now=8)], ttl=5)
        self.assertEqual(r["results"][-2], {"op": "get", "expired": [], "hit": True, "value": 1})
        self.assertEqual(r["results"][-1], {"op": "get", "expired": ["a"], "hit": False, "value": None})

    def test_expiration_order_is_lru(self):
        r = self.sim([put("a"), put("b"), op("get", "a"), resize(1, 5)], ttl=5)
        self.assertEqual(r["results"][-1], {"op": "resize", "expired": ["b", "a"], "evicted": []})
        self.assertEqual(r["entries"], [])

    def test_null_ttl_overrides_default(self):
        r = self.sim([put("a", ttl=None), put("b"), op("get", "a", 10)], ttl=1)
        self.assertEqual(r["entries"], [{"key": "a", "value": 1, "expires_at": None}])
        self.assertEqual(r["results"][-1]["expired"], ["b"])

    def test_zero_ttl_removes_old_without_evicting(self):
        r = self.sim([put("a"), put("b"), put("a", ttl=0), put("c", ttl=0)])
        self.assertEqual(r["entries"], [{"key": "b", "value": 1, "expires_at": None}])
        self.assertEqual(r["results"][-1], {"op": "put", "expired": [], "stored": False, "evicted": []})
        self.assertFalse(self.sim([put("a")], ttl=0)["results"][0]["stored"])

    def test_zero_capacity_and_growth(self):
        r = self.sim([put("a"), resize(1), put("b")], capacity=0)
        self.assertFalse(r["results"][0]["stored"])
        self.assertEqual(r["entries"], [{"key": "b", "value": 1, "expires_at": None}])

    def test_resize_eviction_order(self):
        r = self.sim([put("a"), put("b"), put("c"), op("get", "a"), resize(1)], capacity=3)
        self.assertEqual(r["results"][-1]["evicted"], ["b", "c"])
        self.assertEqual([e["key"] for e in r["entries"]], ["a"])

    def test_delete_and_expired_delete(self):
        r = self.sim([put("a", ttl=2), op("delete", "a", 2), put("b", now=2), op("delete", "b", 2)])
        self.assertEqual(r["results"][1], {"op": "delete", "expired": ["a"], "removed": False})
        self.assertEqual(r["results"][-1], {"op": "delete", "expired": [], "removed": True})

    def test_missing_vs_null(self):
        r = self.sim([op("get"), put("a", None), op("get")])
        self.assertEqual(r["results"][0], {"op": "get", "expired": [], "hit": False, "value": None})
        self.assertEqual(r["results"][-1], {"op": "get", "expired": [], "hit": True, "value": None})

    def test_backward_time_is_atomic(self):
        r = self.sim([put("a", now=2), op("get", now=1)])
        self.assertEqual(r, {"entries": [], "capacity": 2, "results": [], "error": {"code": "invalid_operation", "index": 1}})

    def test_invalid_tail_is_atomic(self):
        for bad in [put("", 1), put("a", ttl=True), resize(-1), op("touch"), dict(op("get"), extra=1), None]:
            with self.subTest(bad=bad):
                self.assertEqual(self.sim([resize(5), put("a"), bad]), {"entries": [], "capacity": 2, "results": [], "error": {"code": "invalid_operation", "index": 2}})

    def test_invalid_config_and_container(self):
        for config in [{}, {"capacity": True, "default_ttl": None}, {"capacity": 2, "default_ttl": -1}, None]:
            with self.subTest(config=config):
                r = call("cache_api", "simulate", [config, None])["result"]
                self.assertEqual(r, {"entries": [], "capacity": None, "results": [], "error": {"code": "invalid_config", "index": None}})
        self.assertEqual(self.sim(None)["error"], {"code": "invalid_operation", "index": None})

    def test_detached_values_and_read_snapshots(self):
        args = [{"capacity": 2, "default_ttl": None}, [put("a", {"xs": []}), op("get"), op("get")]]
        r = call("cache_api", "simulate", args, mutations=[{"target": "result", "path": ["results", 1, "value", "xs"], "op": "append", "value": 7}])
        snap = r["snapshots"][0]
        self.assertEqual(snap["args"], args)
        self.assertEqual(snap["result"]["entries"][0]["value"], {"xs": []})
        self.assertEqual(snap["result"]["results"][2]["value"], {"xs": []})


if __name__ == "__main__":
    unittest.main()
