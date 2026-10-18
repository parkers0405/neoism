"""Public version-aware migration planning API."""
from copy import deepcopy
from schema import valid
from graph import steps

def plan(catalog, installed, targets):
    result = {"steps":[], "versions":deepcopy(installed), "error":None}
    if not valid(catalog, installed, targets):
        result["error"] = {"code":"invalid_input"}
        return result
    by_id = {row["id"]:row for row in catalog}
    if any(key not in by_id for key in targets):
        result["error"] = {"code":"unknown_target"}
        return result
    selected = {key:by_id[key] for key in targets if installed.get(key, 0) < by_id[key]["version"]}
    result["steps"], code = steps(selected)
    if code:
        result["error"] = {"code":code}
        result["steps"] = []
        return result
    result["versions"].update({key:row["version"] for key,row in selected.items()})
    return result
