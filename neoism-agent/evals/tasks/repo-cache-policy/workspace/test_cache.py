import unittest
from cache_api import simulate

class PublicTests(unittest.TestCase):
    def test_empty(self):
        self.assertEqual(simulate({"capacity":2,"default_ttl":None},[]),{"entries":[],"capacity":2,"results":[],"error":None})

    def test_simple(self):
        ops = [{"op":"put","now":0,"key":"a","value":1},{"op":"get","now":0,"key":"a"}]
        self.assertEqual(simulate({"capacity":2,"default_ttl":None},ops)["results"][-1]["value"],1)

    def test_expiry_boundary(self):
        ops = [{"op":"put","now":0,"key":"a","value":1},
               {"op":"get","now":5,"key":"a"}]
        result = simulate({"capacity":2,"default_ttl":5},ops)
        self.assertEqual(result["results"][-1],
                         {"op":"get","expired":["a"],"hit":False,"value":None})

if __name__ == "__main__":
    unittest.main()
