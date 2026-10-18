import unittest
from patch import apply_patch

class PublicTests(unittest.TestCase):
    def test_object_add(self):
        self.assertEqual(apply_patch({}, [{"op":"add","path":"/x","value":1}]), {"ok":True,"document":{"x":1}})
    def test_empty(self):
        self.assertEqual(apply_patch([1], []), {"ok":True,"document":[1]})

if __name__ == "__main__":
    unittest.main()
