"""Stable dependency ordering before and after transaction collapse."""
import heapq

def topological(nodes, dependencies):
    pending = {key:set(dependencies.get(key, ())) & set(nodes) for key in nodes}
    ready = [key for key in nodes if not pending[key]]
    heapq.heapify(ready)
    output = []
    while ready:
        key = heapq.heappop(ready)
        output.append(key)
        for other in pending:
            if key in pending[other]:
                pending[other].remove(key)
                if not pending[other]:
                    heapq.heappush(ready, other)
    return output if len(output) == len(pending) else None

def steps(rows):
    dependencies = {key:set(row["requires"]) & rows.keys() for key,row in rows.items()}
    if topological(rows, dependencies) is None:
        return [], "cycle"
    # A component is named by its smallest member, not its transaction label.
    grouped = {}
    for key,row in rows.items():
        label = ("tx", row["transaction"]) if row["transaction"] is not None else ("id", key)
        grouped.setdefault(label, []).append(key)
    groups = {min(members):set(members) for members in grouped.values()}
    owner = {member:group for group,members in groups.items() for member in members}
    edges = {group:set() for group in groups}
    for key,deps in dependencies.items():
        edges[owner[key]].update(owner[dep] for dep in deps if owner[dep] != owner[key])
    ordered = topological(groups, edges)
    if ordered is None:
        return [], "transaction_cycle"
    output = []
    for group in ordered:
        members = topological(groups[group], dependencies)
        output.append({"transaction": rows[group]["transaction"], "migrations":members})
    return output, None
