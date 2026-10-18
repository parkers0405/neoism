"""Validate JSON planning records without changing them."""
def integer(value):
    return type(value) is int

def validate(capacity, bookings, requests):
    if not integer(capacity) or capacity <= 0 or not isinstance(bookings, list) or not isinstance(requests, list):
        raise ValueError("shape")
    seen = set()
    for records, fields in [(bookings, ("start", "end", "units")), (requests, ("earliest", "latest", "duration", "units"))]:
        for item in records:
            if not isinstance(item, dict):
                raise ValueError("record")
            identity = item.get("id")
            if not isinstance(identity, str) or not identity or identity in seen:
                raise ValueError("id")
            seen.add(identity)
            if not all(integer(item.get(key)) for key in fields):
                raise ValueError("integer")
            if not 1 <= item["units"] <= capacity:
                raise ValueError("units")
            if fields[0] == "start":
                if item["start"] >= item["end"]:
                    raise ValueError("interval")
            elif item["duration"] <= 0 or item["earliest"] + item["duration"] > item["latest"]:
                raise ValueError("window")
