"""Validation shared by the journal transaction and record layer."""

def version(value):
    return type(value) is int and value >= 0

def name(value):
    return isinstance(value, str) and bool(value)

def valid_record(row):
    """Validate one persisted entity, including the tombstone invariant."""
    if not isinstance(row, dict):
        return False
    if set(row) != {"version", "value", "deleted"}:
        return False
    if not version(row["version"]):
        return False
    if type(row["deleted"]) is not bool:
        return False
    if row["deleted"] and row["value"] is not None:
        return False
    return True


def valid_seen(seen):
    """Validate before building a set: JSON IDs can be unhashable."""
    if not isinstance(seen, list):
        return False
    if not all(name(item) for item in seen):
        return False
    return len(set(seen)) == len(seen)


def valid_state(state):
    if not isinstance(state, dict) or set(state) != {"entities", "seen"}:
        return False
    if not isinstance(state["entities"], dict) or not valid_seen(state["seen"]):
        return False
    return all(name(key) and valid_record(row)
               for key, row in state["entities"].items())

def valid_event(event):
    if not isinstance(event, dict):
        return False
    op = event.get("op")
    fields = {"id", "key", "version", "op"}
    if op == "put":
        fields.add("value")
    elif op != "delete":
        return False
    return (set(event) == fields and name(event["id"])
            and name(event["key"]) and version(event["version"]))
