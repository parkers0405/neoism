"""Mutable cache policy state used only inside a transaction."""
from copy import deepcopy

class Cache:
    def __init__(self, config):
        self.capacity = config["capacity"]
        self.default_ttl = config["default_ttl"]
        self.entries = {}

    def apply(self, row):
        op = row["op"]
        result = {"op":op,"expired":[]}
        key = row.get("key")
        if op == "put":
            self.entries.pop(key,None)
            stored = self.capacity > 0
            if stored:
                self.entries[key] = {"key":key,"value":deepcopy(row["value"]),"expires_at":None}
            result.update(stored=stored,evicted=self.trim())
        elif op in ("get","peek"):
            hit = key in self.entries
            result.update(hit=hit,value=deepcopy(self.entries[key]["value"]) if hit else None)
        elif op == "delete":
            result["removed"] = key in self.entries
            self.entries.pop(key,None)
        else:
            self.capacity = row["capacity"]
            result["evicted"] = self.trim()
        return result

    def trim(self):
        removed = []
        while len(self.entries) > self.capacity:
            key = next(reversed(self.entries))
            self.entries.pop(key)
            removed.append(key)
        return removed

    def snapshot(self):
        return deepcopy(list(self.entries.values()))
