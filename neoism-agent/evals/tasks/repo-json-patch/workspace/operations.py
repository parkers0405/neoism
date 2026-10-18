"""Legacy operations; only the original object-edit subset is wired."""
from copy import deepcopy
from pointer import PatchError, tokens, get, edit

def apply(document, operation):
    if not isinstance(operation, dict):
        raise PatchError("invalid")
    op = operation.get("op")
    if op not in ("add","remove","replace","test"):
        raise PatchError("invalid")
    destination = tokens(operation.get("path"))
    if op != "remove" and "value" not in operation:
        raise PatchError("invalid")
    if op == "test":
        if get(document, destination) != operation["value"]:
            raise PatchError("test")
        return document
    return edit(document, destination, op, deepcopy(operation.get("value")))
