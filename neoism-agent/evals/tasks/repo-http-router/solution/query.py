"""Form-style query decoding."""
from urllib.parse import parse_qsl
from patterns import segments

def split_target(target):
    if not isinstance(target, str) or "#" in target:
        raise ValueError("target")
    path, _, query = target.partition("?")
    parts = segments(path)
    values = {}
    for key, value in parse_qsl(query, keep_blank_values=True):
        values.setdefault(key, []).append(value)
    return parts, values
