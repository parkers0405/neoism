"""Deterministic cycle paths and dependency-first scheduling."""
import heapq

def cycle_path(adjacency):
    done = set()
    for root in sorted(adjacency):
        if root in done:
            continue
        path, active = [root], {root: 0}
        stack = [iter(adjacency[root])]
        while stack:
            child = next(stack[-1], None)
            if child is None:
                done.add(path.pop())
                active = {name: index for index, name in enumerate(path)}
                stack.pop()
            elif child in active:
                return path[active[child]:] + [child]
            elif child not in done:
                active[child] = len(path)
                path.append(child)
                stack.append(iter(adjacency[child]))
    return None

def dependents(adjacency):
    reverse = {name: [] for name in adjacency}
    for name in sorted(adjacency):
        for dependency in adjacency[name]:
            reverse[dependency].append(name)
    return reverse

def schedule(adjacency, reverse):
    degree = {name: len(deps) for name, deps in adjacency.items()}
    ready = [name for name, count in degree.items() if count == 0]
    heapq.heapify(ready)
    order = []
    while ready:
        name = heapq.heappop(ready)
        order.append(name)
        for dependent in reverse[name]:
            degree[dependent] -= 1
            if degree[dependent] == 0:
                heapq.heappush(ready, dependent)
    # A second Kahn pass freezes each ready frontier for simultaneous layers.
    degree = {name: len(deps) for name, deps in adjacency.items()}
    frontier = sorted(name for name, count in degree.items() if count == 0)
    layers = []
    while frontier:
        layers.append(frontier)
        following = []
        for name in frontier:
            for dependent in reverse[name]:
                degree[dependent] -= 1
                if degree[dependent] == 0:
                    following.append(dependent)
        frontier = sorted(following)
    return order, layers
