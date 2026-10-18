"""Route grammar and matching, independent of HTTP methods."""
import re

NAME = re.compile(r"[A-Za-z_][A-Za-z0-9_]*\Z")
METHOD = re.compile(r"[A-Za-z]+\Z")

def method(value):
    if not isinstance(value, str) or not METHOD.fullmatch(value):
        raise ValueError("method")
    return value.upper()

def segments(path):
    if not isinstance(path, str) or not path.startswith("/") or "?" in path or "#" in path:
        raise ValueError("path")
    if path == "/":
        return []
    parts = path[1:].split("/")
    if parts[-1] == "":
        parts.pop()
    if not all(parts):
        raise ValueError("empty segment")
    return parts

def compile_path(path):
    parts = segments(path)
    result, names = [], set()
    for index, part in enumerate(parts):
        if part[0] in ":*":
            name = part[1:]
            if not NAME.fullmatch(name) or name in names:
                raise ValueError("capture")
            if part[0] == "*" and index != len(parts) - 1:
                raise ValueError("wildcard position")
            names.add(name)
            result.append((1 if part[0] == ":" else 0, name))
        else:
            if ":" in part or "*" in part:
                raise ValueError("literal")
            result.append((2, part))
    return result

def match(pattern, parts):
    params = {}
    for index, (kind, value) in enumerate(pattern):
        if kind == 0:
            params[value] = "/".join(parts[index:])
            return params
        if index >= len(parts):
            return None
        if kind == 2 and value != parts[index]:
            return None
        if kind == 1:
            params[value] = parts[index]
    return params if len(parts) == len(pattern) else None

def rank(pattern):
    # Completion sentinel beats a zero-width wildcard continuation.
    return tuple(kind for kind, _ in pattern) + (3,)
