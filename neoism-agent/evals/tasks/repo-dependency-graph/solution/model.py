"""Validated adjacency and transitive target selection."""
def prepare(manifest, targets):
    if not isinstance(manifest, dict):
        raise ValueError("manifest")
    if any(not isinstance(name, str) or not name for name in manifest):
        raise ValueError("name")
    adjacency = {}
    for name, dependencies in manifest.items():
        if not isinstance(dependencies, list):
            raise ValueError("dependencies")
        if any(not isinstance(dep, str) or not dep or dep not in manifest for dep in dependencies):
            raise ValueError("dependency")
        adjacency[name] = sorted(set(dependencies))
    if targets is None:
        pending = list(adjacency)
    elif isinstance(targets, list) and all(isinstance(t, str) and t in adjacency for t in targets):
        pending = list(targets)
    else:
        raise ValueError("targets")
    selected = set()
    while pending:
        name = pending.pop()
        if name in selected:
            continue
        selected.add(name)
        pending.extend(adjacency[name])
    return {name: adjacency[name] for name in sorted(selected)}
