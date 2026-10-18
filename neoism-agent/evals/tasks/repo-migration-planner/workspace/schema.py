"""Validate migration catalog and caller state."""
def name(x):
    return isinstance(x, str) and bool(x)

def integer(x, minimum):
    return type(x) is int and x >= minimum

def valid(catalog, installed, targets):
    if not isinstance(catalog, list) or not isinstance(installed, dict):
        return False
    if not isinstance(targets, list) or not all(name(x) for x in targets):
        return False
    if not all(name(k) and integer(v, 0) for k, v in installed.items()):
        return False
    seen = set()
    for row in catalog:
        if not isinstance(row, dict) or set(row) != {"id", "version", "requires", "transaction"}:
            return False
        key, deps, tx = row["id"], row["requires"], row["transaction"]
        if not name(key) or key in seen or not integer(row["version"], 1):
            return False
        if not isinstance(deps, list) or not all(name(x) for x in deps):
            return False
        if len(set(deps)) != len(deps) or (tx is not None and not name(tx)):
            return False
        seen.add(key)
    return True
