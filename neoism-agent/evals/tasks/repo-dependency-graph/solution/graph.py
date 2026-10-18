"""Public JSON facade."""
from model import prepare
from traversal import cycle_path, dependents, schedule

def analyze(manifest, targets):
    try:
        adjacency = prepare(manifest, targets)
    except (ValueError, TypeError):
        return {"error": "invalid"}
    cycle = cycle_path(adjacency)
    if cycle is not None:
        return {"error": "cycle", "cycle": cycle}
    reverse = dependents(adjacency)
    order, layers = schedule(adjacency, reverse)
    return {"order": order, "layers": layers, "dependents": reverse}
