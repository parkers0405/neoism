import unittest
from graph import analyze

class PublicTests(unittest.TestCase):
    def test_single(self):
        self.assertEqual(analyze({"app":[]}, None), {"order":["app"],"layers":[["app"]],"dependents":{"app":[]}})
    def test_empty(self):
        self.assertEqual(analyze({}, []), {"order":[],"layers":[],"dependents":{}})

if __name__ == "__main__":
    unittest.main()
