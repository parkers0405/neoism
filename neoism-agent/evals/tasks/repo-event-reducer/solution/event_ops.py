"""Apply records with idempotency, version barriers, and tombstones."""
from copy import deepcopy
from event_schema import valid_event

def is_newer(entities, event):
    """Both live values and tombstones impose the same version barrier."""
    prior = entities.get(event["key"])
    if prior is None:
        return True
    return event["version"] > prior["version"]


def materialize(event):
    """The journal's payload must not become mutable persisted state."""
    deleted = event["op"] == "delete"
    return {
        "version": event["version"],
        "value": None if deleted else deepcopy(event["value"]),
        "deleted": deleted,
    }


def apply_event(state, event):
    """Return whether a validated event changed an entity.

    Seen IDs are tracked separately from versions: a stale event still becomes
    seen, whereas a replay must never be reinterpreted against another key.
    """
    if not valid_event(event):
        return False
    if event["id"] in state["seen"]:
        return False
    state["seen"].append(event["id"])
    if not is_newer(state["entities"], event):
        return False
    state["entities"][event["key"]] = materialize(event)
    return True
