"""All-or-nothing JSON facade."""
from copy import deepcopy
from operations import apply
from pointer import PatchError

def apply_patch(document, operations):
    if not isinstance(operations, list):
        return {"ok": False, "document": deepcopy(document), "index": None, "error": "invalid"}
    working = deepcopy(document)
    for index, operation in enumerate(operations):
        try:
            working = apply(working, operation)
        except PatchError as error:
            return {"ok": False, "document": working, "index": index, "error": str(error)}
    return {"ok": True, "document": working}
