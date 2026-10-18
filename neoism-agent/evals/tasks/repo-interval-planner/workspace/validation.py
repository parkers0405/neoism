"""Initial record validation, before capacity support."""
def validate(capacity, bookings, requests):
    if type(capacity) is not int or capacity <= 0:
        raise ValueError("capacity")
    for records, fields in [(bookings,("start","end","units")), (requests,("earliest","latest","duration","units"))]:
        if not isinstance(records, list):
            raise ValueError("records")
        for item in records:
            if not isinstance(item, dict) or not isinstance(item.get("id"), str):
                raise ValueError("record")
            if any(type(item.get(key)) is not int for key in fields):
                raise ValueError("fields")
