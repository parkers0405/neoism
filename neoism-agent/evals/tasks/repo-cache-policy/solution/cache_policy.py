"""Controlled-clock LRU policy with detached value snapshots."""
from copy import deepcopy

class Cache:
    def __init__(self, config):
        self.capacity = config["capacity"]
        self.default_ttl = config["default_ttl"]
        self.entries = {}

    def purge(self, now):
        expired = []
        for key,entry in list(self.entries.items()):
            deadline = entry["expires_at"]
            if deadline is not None and deadline <= now:
                expired.append(key)
                del self.entries[key]
        return expired

    def apply(self, row):
        op = row["op"]
        result = {"op":op,"expired":self.purge(row["now"])}
        key = row.get("key")
        if op == "put":
            self.entries.pop(key,None)
            duration = row.get("ttl",self.default_ttl)
            stored = self.capacity > 0 and duration != 0
            if stored:
                self.entries[key] = {
                    "key":key,"value":deepcopy(row["value"]),
                    "expires_at":None if duration is None else row["now"] + duration}
            result.update(stored=stored,evicted=self.trim())
        elif op in ("get","peek"):
            hit = key in self.entries
            result.update(hit=hit,value=deepcopy(self.entries[key]["value"]) if hit else None)
            if hit and op == "get":
                entry = self.entries.pop(key)
                self.entries[key] = entry
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
            key = next(iter(self.entries))
            self.entries.pop(key)
            removed.append(key)
        return removed

    def snapshot(self):
        return deepcopy(list(self.entries.values()))
