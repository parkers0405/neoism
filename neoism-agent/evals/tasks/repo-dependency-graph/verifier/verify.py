"""Trusted deterministic graph acceptance checks via RPC only."""
import unittest
from candidate_client import call

class Acceptance(unittest.TestCase):
    def analyze(self, manifest, targets=None):
        args=[manifest,targets]
        reply=call("graph","analyze",args)
        self.assertEqual(reply["args_after"],args)
        return reply["result"]
    def test_dependency_before_name(self):
        self.assertEqual(self.analyze({"a":["z"],"z":[]}),{"order":["z","a"],"layers":[["z"],["a"]],"dependents":{"a":[],"z":["a"]}})
    def test_dynamic_ready_order(self):
        result=self.analyze({"a":[],"b":["a"],"z":[]})
        self.assertEqual(result["order"],["a","b","z"])
        self.assertEqual(result["layers"],[["a","z"],["b"]])
    def test_transitive_target_closure(self):
        result=self.analyze({"app":["mid"],"mid":["base"],"base":[],"other":[]},["app"])
        self.assertEqual(result,{"order":["base","mid","app"],"layers":[["base"],["mid"],["app"]],"dependents":{"app":[],"base":["mid"],"mid":["app"]}})
    def test_diamond(self):
        result=self.analyze({"app":["b","c"],"b":["d"],"c":["d"],"d":[]})
        self.assertEqual(result["order"],["d","b","c","app"])
        self.assertEqual(result["layers"],[["d"],["b","c"],["app"]])
        self.assertEqual(result["dependents"]["d"],["b","c"])
    def test_duplicates(self):
        self.assertEqual(self.analyze({"a":["z","z"],"z":[]},["a","a"]),self.analyze({"z":[],"a":["z"]}))
    def test_empty_selection(self):
        self.assertEqual(self.analyze({"a":["a"]},[]),{"order":[],"layers":[],"dependents":{}})
    def test_self_cycle(self):
        self.assertEqual(self.analyze({"a":["a"]}),{"error":"cycle","cycle":["a","a"]})
    def test_cycle_stack_suffix(self):
        self.assertEqual(self.analyze({"a":["b"],"b":["c"],"c":["b"]}),{"error":"cycle","cycle":["b","c","b"]})
    def test_cycle_sorted_traversal(self):
        self.assertEqual(self.analyze({"z":["z"],"a":["c","b"],"b":["a"],"c":["a"]}),{"error":"cycle","cycle":["a","b","a"]})
    def test_cycle_not_rotated(self):
        self.assertEqual(self.analyze({"a":["z"],"z":["b"],"b":["z"]}),{"error":"cycle","cycle":["z","b","z"]})
    def test_unselected_cycle_ignored(self):
        self.assertEqual(self.analyze({"a":[],"z":["z"]},["a"]),{"order":["a"],"layers":[["a"]],"dependents":{"a":[]}})
    def test_invalid_manifest(self):
        for manifest in [[],{"a":"b"},{"":[]},{"a":["missing"]},{"a":[True]},{"a":[[]]}]:
            with self.subTest(manifest=manifest):
                self.assertEqual(self.analyze(manifest),{"error":"invalid"})
    def test_invalid_targets_and_unused(self):
        for targets in ["a",["missing"],[1],[[]]]:
            with self.subTest(targets=targets):
                self.assertEqual(self.analyze({"a":[]},targets),{"error":"invalid"})
        self.assertEqual(self.analyze({"a":[],"bad":["missing"]},["a"]),{"error":"invalid"})
    def test_sort_case_and_direct_dependents(self):
        result=self.analyze({"x":["A"],"A":[],"a":["x"],"☃":[]})
        self.assertEqual(result["order"],["A","x","a","☃"])
        self.assertEqual(result["dependents"],{"A":["x"],"a":[],"x":["a"],"☃":[]})
    def test_insertion_order_invariance(self):
        manifest={"a":["d","c"],"b":["c"],"c":[],"d":[]}
        reverse={k:list(reversed(v)) for k,v in reversed(list(manifest.items()))}
        self.assertEqual(self.analyze(manifest),self.analyze(reverse))
    def test_chain_property(self):
        for length in range(2,9):
            names=["n"+str(i) for i in range(length)]
            manifest={name:([names[i+1]] if i+1<length else []) for i,name in enumerate(names)}
            result=self.analyze(manifest,[names[0]])
            self.assertEqual(result["order"],list(reversed(names)))
            self.assertEqual(result["layers"],[[name] for name in reversed(names)])

if __name__ == "__main__":
    unittest.main()
