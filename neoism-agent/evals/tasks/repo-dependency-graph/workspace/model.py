"""Prototype manifest validation and direct target selection."""
def prepare(manifest, targets):
    if not isinstance(manifest, dict):
        raise ValueError("manifest")
    adjacency = {}
    for name, dependencies in manifest.items():
        if not isinstance(name, str) or not name or not isinstance(dependencies, list):
            raise ValueError("node")
        if any(not isinstance(dep, str) or dep not in manifest for dep in dependencies):
            raise ValueError("dependency")
        adjacency[name] = sorted(set(dependencies))
    if targets is None:
        return adjacency
    if not isinstance(targets, list) or any(not isinstance(t,str) or t not in adjacency for t in targets):
        raise ValueError("targets")
    return {name:adjacency[name] for name in sorted(set(targets))}
