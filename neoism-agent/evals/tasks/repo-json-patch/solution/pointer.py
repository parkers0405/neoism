"""Strict JSON pointer parsing and container access."""
import re

class PatchError(Exception):
    pass

def tokens(pointer):
    if not isinstance(pointer, str) or (pointer and not pointer.startswith("/")):
        raise PatchError("invalid")
    if not pointer:
        return []
    result = []
    for token in pointer[1:].split("/"):
        if re.search(r"~(?![01])", token):
            raise PatchError("invalid")
        result.append(token.replace("~1", "/").replace("~0", "~"))
    return result

def index(token, length, adding=False):
    if adding and token == "-":
        return length
    if not re.fullmatch(r"0|[1-9][0-9]*", token):
        raise PatchError("path")
    # Avoid unbounded integer conversion for an obviously out-of-range index.
    if len(token) > len(str(length)):
        raise PatchError("path")
    value = int(token)
    if value > length or (value == length and not adding):
        raise PatchError("path")
    return value

def get(document, parts):
    current = document
    for token in parts:
        if isinstance(current, dict):
            if token not in current:
                raise PatchError("path")
            current = current[token]
        elif isinstance(current, list):
            current = current[index(token, len(current))]
        else:
            raise PatchError("path")
    return current

def edit(document, parts, mode, value=None):
    if not parts:
        return None if mode == "remove" else value
    parent = get(document, parts[:-1])
    token = parts[-1]
    if isinstance(parent, dict):
        if mode != "add" and token not in parent:
            raise PatchError("path")
        if mode == "remove":
            del parent[token]
        else:
            parent[token] = value
    elif isinstance(parent, list):
        position = index(token, len(parent), mode == "add")
        if mode == "add":
            parent.insert(position, value)
        elif mode == "remove":
            parent.pop(position)
        else:
            parent[position] = value
    else:
        raise PatchError("path")
    return document
