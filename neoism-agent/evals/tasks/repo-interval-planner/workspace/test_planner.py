import unittest
from planner import plan

class PublicTests(unittest.TestCase):
    def test_empty(self):
        self.assertEqual(plan(2, [], []), {"accepted":[],"rejected":[],"profile":[],"peak":0})
    def test_one(self):
        result=plan(1, [], [{"id":"x","earliest":0,"latest":2,"duration":2,"units":1}])
        self.assertEqual(result["accepted"], [{"id":"x","start":0,"end":2,"units":1}])

if __name__ == "__main__":
    unittest.main()
