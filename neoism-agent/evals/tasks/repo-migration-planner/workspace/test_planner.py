import unittest
from planner import plan

class PublicTests(unittest.TestCase):
    def test_empty(self):
        self.assertEqual(plan([],{},[]), {"steps":[],"versions":{},"error":None})

    def test_independent(self):
        row = {"id":"a","version":1,"requires":[],"transaction":None}
        self.assertEqual(plan([row],{},["a"])["versions"], {"a":1})

    def test_target_pulls_dependencies(self):
        rows = [{"id":"a","version":1,"requires":[],"transaction":None},
                {"id":"b","version":1,"requires":["a"],"transaction":None}]
        self.assertEqual(plan(rows,{},["b"])["versions"],{"a":1,"b":1})

if __name__ == "__main__":
    unittest.main()
