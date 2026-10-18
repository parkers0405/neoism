"""Patch operations, isolated from transaction formatting."""
from copy import deepcopy
from pointer import PatchError, tokens, get, edit

def equal(left, right):
    if isinstance(left, bool) or isinstance(right, bool):
        return type(left) is type(right) and left == right
    if isinstance(left, (int, float)) and isinstance(right, (int, float)):
        return left == right
    if type(left) is not type(right):
        return False
    if isinstance(left, list):
        return len(left) == len(right) and all(equal(a, b) for a, b in zip(left, right))
    if isinstance(left, dict):
        return left.keys() == right.keys() and all(equal(left[k], right[k]) for k in left)
    return left == right

def apply(document, operation):
    if not isinstance(operation, dict):
        raise PatchError("invalid")
    op = operation.get("op")
    if not isinstance(op, str) or op not in ("add", "remove", "replace", "test", "copy", "move"):
        raise PatchError("invalid")
    destination = tokens(operation.get("path"))
    if op in ("add", "replace", "test") and "value" not in operation:
        raise PatchError("invalid")
    source = tokens(operation.get("from")) if op in ("copy", "move") else None
    if op == "test":
        if not equal(get(document, destination), operation["value"]):
            raise PatchError("test")
        return document
    if op in ("copy", "move"):
        value = deepcopy(get(document, source))
        if op == "move":
            if source == destination:
                return document
            if destination[:len(source)] == source:
                raise PatchError("path")
            document = edit(document, source, "remove")
        return edit(document, destination, "add", value)
    return edit(document, destination, op, deepcopy(operation.get("value")))
