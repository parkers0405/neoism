"""Prototype traversal: a reachability check and sorted build list."""
def cycle_path(adjacency):
    for root in sorted(adjacency):
        pending = list(adjacency[root])
        visited = set()
        while pending:
            name = pending.pop()
            if name == root:
                return sorted(adjacency) + [sorted(adjacency)[0]]
            if name not in visited:
                visited.add(name)
                pending.extend(adjacency[name])
    return None

def dependents(adjacency):
    return {name:sorted(other for other,deps in adjacency.items() if name in deps)
            for name in adjacency}

def schedule(adjacency, reverse):
    names = sorted(adjacency)
    return names, [names] if names else []
