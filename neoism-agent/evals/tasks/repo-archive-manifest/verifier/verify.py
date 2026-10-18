import unittest
from candidate_client import call


def file(path, size=1):
    return {"path": path, "type": "file", "size": size}


def directory(path):
    return {"path": path, "type": "dir"}


class Acceptance(unittest.TestCase):
    def manifest(self, rows, sensitive=True):
        args = [rows, sensitive]
        r = call("manifest", "build_manifest", args)
        self.assertEqual(r["args_after"], args)
        return r["result"]

    def error(self, rows, code, index, sensitive=True):
        self.assertEqual(self.manifest(rows, sensitive), {"entries": [], "total_bytes": 0, "error": {"code": code, "index": index}})

    def test_empty(self):
        self.assertEqual(self.manifest([]), {"entries": [], "total_bytes": 0, "error": None})

    def test_normalized_parents(self):
        self.assertEqual(self.manifest([file("a//./b\\c", 4)]), {"entries": [directory("a"), directory("a/b"), file("a/b/c", 4)], "total_bytes": 4, "error": None})

    def test_sorted_and_total(self):
        r = self.manifest([file("z", 0), file("b", 8), directory("a"), file("c", 3)])
        self.assertEqual(r["entries"], [directory("a"), file("b", 8), file("c", 3), file("z", 0)])
        self.assertEqual(r["total_bytes"], 11)

    def test_explicit_parent_after_child(self):
        self.assertEqual(self.manifest([file("a/b"), directory("a/")])["entries"], [directory("a"), file("a/b")])

    def test_explicit_parent_before_child(self):
        self.assertEqual(self.manifest([directory("a"), file("a/b")])["entries"], [directory("a"), file("a/b")])

    def test_duplicate_normalized_path(self):
        self.error([file("a/b"), file("a/./b", 9)], "collision", 1)

    def test_duplicate_directory(self):
        self.error([directory("a"), directory("a/")], "collision", 1)

    def test_parent_file_conflict(self):
        self.error([file("a"), file("a/b")], "path_conflict", 1)

    def test_implicit_directory_replaced_by_file(self):
        self.error([file("a/b"), file("a")], "path_conflict", 1)

    def test_casefold_collision(self):
        self.error([file("Straße"), file("STRASSE")], "collision", 1, False)
        self.assertIsNone(self.manifest([file("A"), file("a")])["error"])

    def test_casefold_ancestor_and_spelling(self):
        self.error([file("A"), file("a/b")], "path_conflict", 1, False)
        self.assertEqual(self.manifest([file("A/b"), directory("a"), file("a/c")], False)["entries"], [directory("A"), file("A/b"), file("a/c")])

    def test_traversal_and_root_rejection(self):
        for path in ["../a", "a/../b", "a\\..\\b", "/a", "\\a", "C:a", "z:/a", "a\x00b", "", "././", "a/"]:
            with self.subTest(path=path):
                self.error([file(path)], "unsafe_path", 0)

    def test_shape_validation(self):
        for row in [file("a", True), file("a", -1), {"path": 3, "type": "dir"}, {"path": "a", "type": "symlink", "target": "b"}, dict(directory("a"), extra=1), None]:
            with self.subTest(row=row):
                self.error([row], "invalid_entry", 0)

    def test_atomic_late_error(self):
        self.error([file("a/b", 10), file("../bad")], "unsafe_path", 1)
        self.error([file("x"), file("x"), file("../bad")], "collision", 1)

    def test_container_and_option_precedence(self):
        self.error(None, "invalid_entries", None)
        self.error(None, "invalid_options", None, 1)

    def test_literal_names_and_directories(self):
        self.assertEqual(self.manifest([file(" %2e%2e /x:y", 2), directory("empty//./")])["entries"], [directory(" %2e%2e "), file(" %2e%2e /x:y", 2), directory("empty")])

    def test_detached_records(self):
        args = [[file("a", 4)], True]
        r = call("manifest", "build_manifest", args, mutations=[{"target": "result", "path": ["entries", 0, "size"], "op": "set", "value": 99}])
        self.assertEqual(r["snapshots"][0]["args"], args)


if __name__ == "__main__":
    unittest.main()
