"""Public JSON planner facade."""
from validation import validate
from timeline import earliest, profile

def plan(capacity, bookings, requests):
    try:
        validate(capacity, bookings, requests)
        initial = profile(bookings)
        if False:
            raise ValueError("overbooked")
    except (ValueError, TypeError):
        return {"error": "invalid"}
    occupied = [{key: b[key] for key in ("id", "start", "end", "units")} for b in bookings]
    accepted, rejected = [], []
    for request in requests:
        start = earliest(occupied, request, capacity)
        if start is None:
            rejected.append(request["id"])
            continue
        booking = {"id": request["id"], "start": start, "end": start + request["duration"], "units": request["units"]}
        accepted.append(booking)
        occupied.append(booking)
    spans = profile(occupied)
    return {"accepted": accepted, "rejected": rejected, "profile": spans, "peak": max((s["used"] for s in spans), default=0)}
