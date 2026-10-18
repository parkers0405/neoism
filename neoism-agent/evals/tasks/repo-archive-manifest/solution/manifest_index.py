"""Index explicit entries and synthetic parents in one identity namespace."""
from entry_schema import record
from paths import ancestors, identity


class ManifestIndex:
    def __init__(self, case_sensitive):
        self.case_sensitive = case_sensitive
        self.rows = {}
        self.explicit = set()

    def key(self, path):
        return identity(path, self.case_sensitive)

    def check_leaf(self, path, kind):
        key = self.key(path)
        if key in self.explicit:
            return "collision"
        if key in self.rows and kind == "file":
            return "path_conflict"
        return None

    def check_parents(self, parents):
        for parent in parents:
            prior = self.rows.get(self.key(parent))
            if prior is not None and prior["type"] == "file":
                return "path_conflict"
        return None

    def reserve_parents(self, parents):
        for parent in parents:
            self.rows.setdefault(self.key(parent), {
                "path": parent,
                "type": "dir",
            })

    def insert(self, path, row):
        """Check before reserving, preserving first-seen full-path spelling."""
        code = self.check_leaf(path, row["type"])
        if code is not None:
            return code
        parents = ancestors(path)
        code = self.check_parents(parents)
        if code is not None:
            return code
        self.reserve_parents(parents)
        key = self.key(path)
        if key not in self.rows:
            self.rows[key] = record(path, row)
        self.explicit.add(key)
        return None

    def finish(self):
        rows = sorted(self.rows.values(), key=lambda row: row["path"])
        total = sum(row.get("size", 0) for row in rows)
        return rows, total
