"""Exact JSON schemas for deterministic cache operations."""
def natural(x):
    return type(x) is int and x >= 0

def ttl(x):
    return x is None or natural(x)

def valid_config(config):
    return (isinstance(config,dict) and set(config) == {"capacity","default_ttl"}
            and natural(config["capacity"]) and ttl(config["default_ttl"]))

def valid_operation(row, previous):
    if not isinstance(row,dict) or not natural(row.get("now")):
        return False
    if row["now"] < previous:
        return False
    op = row.get("op")
    fields = {"op","now"}
    if op in ("get","peek","delete","put"):
        if not isinstance(row.get("key"),str) or not row["key"]:
            return False
        fields.add("key")
        if op == "put":
            fields.add("value")
            if "ttl" in row:
                fields.add("ttl")
                if not ttl(row["ttl"]):
                    return False
    elif op == "resize":
        fields.add("capacity")
        if not natural(row.get("capacity")):
            return False
    else:
        return False
    return set(row) == fields
