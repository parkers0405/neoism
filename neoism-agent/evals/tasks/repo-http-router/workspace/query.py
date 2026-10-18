"""Legacy URL splitter with raw query fields."""
from patterns import segments

def split_target(target):
    if not isinstance(target, str):
        raise ValueError("target")
    path, _, text = target.partition("?")
    query = {}
    for field in text.split("&"):
        if field:
            key, _, value = field.partition("=")
            query[key] = [value]
    return segments(path), query
