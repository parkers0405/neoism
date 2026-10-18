"""Validate first, then commit a detached batch transaction."""
from copy import deepcopy
from event_schema import valid_state, valid_event
from event_ops import apply_event

def reduce_events(state, events):
    result = {"state": deepcopy(state), "applied": [], "ignored": [], "error": None}
    if not valid_state(state):
        result["error"] = {"code": "invalid_state", "index": None}
        return result
    if not isinstance(events, list):
        result["error"] = {"code": "invalid_event", "index": None}
        return result
    for index, event in enumerate(events):
        if not valid_event(event):
            result["error"] = {"code": "invalid_event", "index": index}
            return result
    for event in events:
        bucket = "applied" if apply_event(result["state"], event) else "ignored"
        result[bucket].append(event["id"])
    return result
