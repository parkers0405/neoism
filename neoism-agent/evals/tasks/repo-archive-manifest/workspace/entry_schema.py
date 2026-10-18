"""Archive entry shape validation and detached record creation."""
def valid_entry(row):
    if not isinstance(row, dict) or not isinstance(row.get("path"), str):
        return False
    if row.get("type") == "dir":
        return set(row) == {"path", "type"}
    if row.get("type") == "file":
        return (set(row) == {"path", "type", "size"}
                and type(row["size"]) is int and row["size"] >= 0)
    return False

def record(path, row):
    result = {"path":path, "type":row["type"]}
    if row["type"] == "file":
        result["size"] = row["size"]
    return result
