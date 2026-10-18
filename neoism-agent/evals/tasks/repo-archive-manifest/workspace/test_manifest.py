import unittest
from manifest import build_manifest

class PublicTests(unittest.TestCase):
    def test_empty(self):
        self.assertEqual(build_manifest([]),{"entries":[],"total_bytes":0,"error":None})

    def test_flat_file(self):
        self.assertEqual(build_manifest([{"path":"a","type":"file","size":3}])["total_bytes"],3)

    def test_missing_parent_is_synthesized(self):
        rows = [{"path":"a/b","type":"file","size":3}]
        self.assertEqual(build_manifest(rows)["entries"],
                         [{"path":"a","type":"dir"},rows[0]])

if __name__ == "__main__":
    unittest.main()
