import unittest
from engine import evaluate

class PublicTests(unittest.TestCase):
    def test_add(self):
        self.assertEqual(evaluate("2 + 3", {}), {"value":5})
    def test_variable(self):
        self.assertEqual(evaluate("cost * count", {"cost":2,"count":3}), {"value":6})

if __name__ == "__main__":
    unittest.main()
