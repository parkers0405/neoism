import unittest
from reducer import reduce_events

class PublicTests(unittest.TestCase):
    def test_empty(self):
        self.assertIsNone(reduce_events({"entities": {}, "seen": []}, [])["error"])

    def test_put(self):
        event = {"id":"a", "key":"x", "version":0, "op":"put", "value":1}
        self.assertEqual(reduce_events({"entities":{},"seen":[]}, [event])["applied"], ["a"])

    def test_replay_is_ignored(self):
        event = {"id":"a","key":"x","version":1,"op":"put","value":1}
        result = reduce_events({"entities":{},"seen":[]},[event,event])
        self.assertEqual(result["ignored"],["a"])
        self.assertEqual(result["state"]["seen"],["a"])

if __name__ == "__main__":
    unittest.main()
