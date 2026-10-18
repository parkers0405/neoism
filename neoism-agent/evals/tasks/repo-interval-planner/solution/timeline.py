"""Sweep-line capacity primitives."""
def profile(bookings):
    events = {}
    for booking in bookings:
        start, end, units = booking["start"], booking["end"], booking["units"]
        events[start] = events.get(start, 0) + units
        events[end] = events.get(end, 0) - units
    result, used, previous = [], 0, None
    for time in sorted(events):
        if previous is not None and previous < time and used:
            if result and result[-1]["end"] == previous and result[-1]["used"] == used:
                result[-1]["end"] = time
            else:
                result.append({"start": previous, "end": time, "used": used})
        used += events[time]
        previous = time
    return result

def fits(spans, start, end, units, capacity):
    return all(span["used"] + units <= capacity
               for span in spans if span["start"] < end and start < span["end"])

def earliest(bookings, request, capacity):
    start = request["earliest"]
    last = request["latest"] - request["duration"]
    # Feasibility can improve only at the end of an occupied interval.
    candidates = {start}
    candidates.update(b["end"] for b in bookings if start <= b["end"] <= last)
    spans = profile(bookings)
    for candidate in sorted(candidates):
        if fits(spans, candidate, candidate + request["duration"], request["units"], capacity):
            return candidate
    return None
