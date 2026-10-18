"""Atomic dependency closure and version planning."""
from copy import deepcopy
from schema import valid
from graph import steps

def plan(catalog, installed, targets):
    result = {"steps":[], "versions":deepcopy(installed), "error":None}
    def fail(code):
        result["error"] = {"code":code}
        return result
    if not valid(catalog, installed, targets):
        return fail("invalid_input")
    by_id = {row["id"]:row for row in catalog}
    if any(key not in by_id for key in targets):
        return fail("unknown_target")
    selected = {}
    pending = list(targets)
    missing = False
    while pending:
        key = pending.pop()
        if key not in by_id:
            missing = True
            continue
        row = by_id[key]
        if key in selected or installed.get(key, 0) >= row["version"]:
            continue
        selected[key] = row
        pending.extend(row["requires"])
    if missing:
        return fail("missing_dependency")
    batches, code = steps(selected)
    if code:
        return fail(code)
    result["steps"] = batches
    result["versions"].update({key:row["version"] for key,row in selected.items()})
    return result
