"""Legacy slash-separated traversal (not yet escaped JSON pointers)."""
class PatchError(Exception):
    pass

def tokens(pointer):
    if not isinstance(pointer, str) or (pointer and not pointer.startswith("/")):
        raise PatchError("invalid")
    return pointer[1:].split("/") if pointer else []

def index(token, length, adding=False):
    if adding and token == "-":
        return length
    try:
        value = int(token)
    except ValueError:
        raise PatchError("path")
    if value < 0 or value > length or (value == length and not adding):
        raise PatchError("path")
    return value

def get(document, parts):
    current = document
    for token in parts:
        if isinstance(current, dict) and token in current:
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
        if mode == "remove":
            parent.pop(position)
        elif position == len(parent):
            parent.append(value)
        else:
            parent[position] = value
    else:
        raise PatchError("path")
    return document
