"""Legacy per-booking display; does not aggregate simultaneous load."""
def profile(bookings):
    return [{"start":b["start"],"end":b["end"],"used":b["units"]}
            for b in sorted(bookings, key=lambda b:(b["start"], b["end"]))]

def fits(spans, start, end, units, capacity):
    # Prototype checks each booking in isolation, not the combined timeline.
    return all(span["used"] + units <= capacity for span in spans
               if span["start"] < end and start < span["end"])

def earliest(bookings, request, capacity):
    # Legacy placement ignores later release boundaries.
    start = request["earliest"]
    if fits(profile(bookings), start, start + request["duration"], request["units"], capacity):
        return start
    return None
