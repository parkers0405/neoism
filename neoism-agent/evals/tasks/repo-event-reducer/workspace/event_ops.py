"""Apply records to the mutable transaction copy."""
from copy import deepcopy
from event_schema import valid_event

def apply_event(state, event):
    if not valid_event(event):
        return False
    state["seen"].append(event["id"])
    if event["op"] == "delete":
        state["entities"].pop(event["key"], None)
    else:
        state["entities"][event["key"]] = {
            "version": event["version"], "value": deepcopy(event["value"]),
            "deleted": False}
    return True
